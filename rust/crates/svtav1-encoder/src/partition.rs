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
    /// C `seq_header.sb_mi_size` — superblock size in MI (4px) units, 16 at
    /// SB64 and 32 at SB128 (task #91). Feeds the intra availability tables
    /// (`intra_edge::has_top_right` / `has_bottom_left`), which index blocks
    /// by `mi & (sb_mi_size - 1)`. Defaults to 16 so every pre-SB128 caller
    /// is byte-identical by construction; `pipeline` overrides it from the
    /// derived SB size.
    pub sb_mi_size: usize,
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
            sb_mi_size: 16,
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
            sb_mi_size: 16,
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
) -> (alloc::vec::Vec<u8>, alloc::vec::Vec<u8>, u8, bool, bool) {
    extract_neighbors_tiled(recon, stride, abs_x, abs_y, width, height, 0)
}

/// Extract prediction neighbors for a block at absolute position
/// (abs_x, abs_y) directly from the reconstruction buffer.
///
/// The buffer is the live frame (or SB) reconstruction written in coding
/// order, so above/left pixels — including those inside the current
/// superblock — are always current, exactly as the decoder sees them.
///
/// Unavailable edges are filled with the C decoder's rules
/// (libaom reconintra.c build_intra_predictors):
/// - above row missing: fill with left_ref[0] if the left column exists,
///   else 127
/// - left column missing: fill with above_ref[0] if the above row exists,
///   else 129
/// - top-left: above_ref[-1] if both exist; above_ref[0] if only above;
///   left_ref[0] if only left; 128 if neither
/// - samples past the reconstructed area extend the last available sample
///
/// Filling with anything else (previously a flat 128) makes the encoder
/// predict from pixels the decoder never sees: an edge V_PRED block coded
/// against pred=128 decodes against pred=left_ref[0], corrupting the
/// reconstruction by the difference. `tile_top` extends this same rule to
/// tile-row boundaries (see [`extract_neighbors`] for the untiled form).
pub(crate) fn extract_neighbors_tiled(
    recon: &[u8],
    stride: usize,
    abs_x: usize,
    abs_y: usize,
    width: usize,
    height: usize,
    tile_top: usize,
) -> (alloc::vec::Vec<u8>, alloc::vec::Vec<u8>, u8, bool, bool) {
    // Task #86: `tile_top` is this plane's tile-row origin (0 = single
    // tile row / unchanged pre-#86 behavior). AV1 intra prediction never
    // crosses a tile boundary — a block at a TILE's own top row has no
    // "above" neighbor even when it is NOT the frame's top row (the
    // frame-absolute `abs_y > 0` this used to read was correct only
    // because every block used to be in the one tile spanning the whole
    // frame). Reading real pixel data across a tile-row boundary would
    // desync a conforming decoder — it reconstructs each tile
    // independently and has no such pixels to read either.
    let has_above = abs_y > tile_top;
    let has_left = abs_x > 0;

    // C left_ref[0] / above_ref[0]: the first sample of each neighbor edge.
    let left_ref0 = if has_left {
        recon.get(abs_y * stride + abs_x - 1).copied()
    } else {
        None
    };
    let above_ref0 = if has_above {
        recon.get((abs_y - 1) * stride + abs_x).copied()
    } else {
        None
    };

    let above: alloc::vec::Vec<u8> = if has_above {
        let row = abs_y - 1;
        let mut v = alloc::vec::Vec::with_capacity(width);
        let mut last = above_ref0.unwrap_or(127);
        for i in 0..width {
            let x = abs_x + i;
            let idx = row * stride + x;
            if x < stride && idx < recon.len() {
                last = recon[idx];
            }
            // else: extend the last available sample, like C
            v.push(last);
        }
        v
    } else {
        alloc::vec![left_ref0.unwrap_or(127); width]
    };

    let left: alloc::vec::Vec<u8> = if has_left {
        let col = abs_x - 1;
        let mut v = alloc::vec::Vec::with_capacity(height);
        let mut last = left_ref0.unwrap_or(129);
        for i in 0..height {
            let idx = (abs_y + i) * stride + col;
            if idx < recon.len() {
                last = recon[idx];
            }
            v.push(last);
        }
        v
    } else {
        alloc::vec![above_ref0.unwrap_or(129); height]
    };

    let top_left = if has_above && has_left {
        recon
            .get((abs_y - 1) * stride + abs_x - 1)
            .copied()
            .unwrap_or(128)
    } else if has_above {
        above_ref0.unwrap_or(128)
    } else if has_left {
        left_ref0.unwrap_or(128)
    } else {
        128
    };

    (above, left, top_left, has_above, has_left)
}

/// High-bit-depth (u16) mirror of [`extract_neighbors`] for the bd10 u16 MD
/// path (task #94). Identical neighbour-availability + edge-extend rules; the
/// only bit-depth dependence is the C `build_intra_predictors_high` fallback
/// fills — `base = 128 << (bd - 8)` (512 at bd10), so a missing above row with
/// no left is `base - 1` (511) and a missing left column with no above is
/// `base + 1` (513), top-left-neither is `base` (512). At bd == 8 this reduces
/// to the exact 127/129/128 the u8 path uses (verified in tests), so the u8
/// path is untouched. `tile_top == 0` (single tile row) matches the funnel's
/// current scope.
pub(crate) fn extract_neighbors_hbd(
    recon: &[u16],
    stride: usize,
    abs_x: usize,
    abs_y: usize,
    width: usize,
    height: usize,
    bd: u8,
) -> (alloc::vec::Vec<u16>, alloc::vec::Vec<u16>, u16, bool, bool) {
    let base: u16 = 128u16 << (bd - 8);
    let has_above = abs_y > 0;
    let has_left = abs_x > 0;

    let left_ref0 = if has_left {
        recon.get(abs_y * stride + abs_x - 1).copied()
    } else {
        None
    };
    let above_ref0 = if has_above {
        recon.get((abs_y - 1) * stride + abs_x).copied()
    } else {
        None
    };

    let above: alloc::vec::Vec<u16> = if has_above {
        let row = abs_y - 1;
        let mut v = alloc::vec::Vec::with_capacity(width);
        let mut last = above_ref0.unwrap_or(base - 1);
        for i in 0..width {
            let x = abs_x + i;
            let idx = row * stride + x;
            if x < stride && idx < recon.len() {
                last = recon[idx];
            }
            v.push(last);
        }
        v
    } else {
        alloc::vec![left_ref0.unwrap_or(base - 1); width]
    };

    let left: alloc::vec::Vec<u16> = if has_left {
        let col = abs_x - 1;
        let mut v = alloc::vec::Vec::with_capacity(height);
        let mut last = left_ref0.unwrap_or(base + 1);
        for i in 0..height {
            let idx = (abs_y + i) * stride + col;
            if idx < recon.len() {
                last = recon[idx];
            }
            v.push(last);
        }
        v
    } else {
        alloc::vec![above_ref0.unwrap_or(base + 1); height]
    };

    let top_left = if has_above && has_left {
        recon
            .get((abs_y - 1) * stride + abs_x - 1)
            .copied()
            .unwrap_or(base)
    } else if has_above {
        above_ref0.unwrap_or(base)
    } else if has_left {
        left_ref0.unwrap_or(base)
    } else {
        base
    };

    (above, left, top_left, has_above, has_left)
}

/// Save a rectangular region of the reconstruction buffer.
fn save_region(
    recon: &[u8],
    stride: usize,
    abs_x: usize,
    abs_y: usize,
    width: usize,
    height: usize,
) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec![0u8; width * height];
    for r in 0..height {
        let src = (abs_y + r) * stride + abs_x;
        out[r * width..r * width + width].copy_from_slice(&recon[src..src + width]);
    }
    out
}

/// Restore a rectangular region of the reconstruction buffer.
fn restore_region(
    recon: &mut [u8],
    stride: usize,
    abs_x: usize,
    abs_y: usize,
    width: usize,
    height: usize,
    saved: &[u8],
) {
    for r in 0..height {
        let dst = (abs_y + r) * stride + abs_x;
        recon[dst..dst + width].copy_from_slice(&saved[r * width..r * width + width]);
    }
}

/// Recursive partition tree for spec-conformant bitstream encoding.
///
/// AV1 requires encoding partition syntax in recursive tree order:
/// write the partition type at each node, then recurse into children.
/// This tree captures the full partition structure for an SB.
#[derive(Debug, Clone)]
pub enum PartitionTree {
    /// Leaf node: PARTITION_NONE — encode block directly.
    Leaf(BlockDecision),
    /// Internal node: partition type + child sub-trees.
    Split {
        partition_type: PartitionType,
        width: u16,
        height: u16,
        children: alloc::vec::Vec<PartitionTree>,
    },
}

impl PartitionTree {
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
}

/// Per-block encoding decision record for bitstream encoding.
#[derive(Debug, Clone)]
pub struct BlockDecision {
    /// Partition type that produced this block.
    pub partition_type: PartitionType,
    /// Whether this block uses inter prediction.
    pub is_inter: bool,
    /// Intra prediction mode index (0-12 for AV1 modes).
    pub intra_mode: u8,
    /// Transform type used for the residual (C TxType index; 0 = DCT_DCT).
    /// MUST match what the bitstream signals or the decoder inverse-
    /// transforms with the wrong basis.
    pub tx_type: u8,
    /// Motion vector (for inter blocks).
    pub mv: svtav1_types::motion::Mv,
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
            intra_mode: 0,
            tx_type: 0,
            mv: svtav1_types::motion::Mv::ZERO,
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

/// AV1 partition type for bitstream encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum PartitionType {
    #[default]
    None = 0,
    Horz = 1,
    Vert = 2,
    Split = 3,
    HorzA = 4,
    HorzB = 5,
    VertA = 6,
    VertB = 7,
    Horz4 = 8,
    Vert4 = 9,
}

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
    (svtav1_entropy::context::partition_symbol_cost(width, 0, sym as usize) + 1) >> 1
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

/// Uses default config (all features enabled). No frame context (mid-gray neighbors).
pub fn partition_search(
    src: &[u8],
    src_stride: usize,
    recon: &mut [u8],
    recon_stride: usize,
    width: usize,
    height: usize,
    qindex: u8,
    lambda: u64,
    max_depth: u32,
) -> PartitionResult {
    partition_search_with_config(
        src,
        src_stride,
        recon,
        recon_stride,
        width,
        height,
        qindex,
        lambda,
        max_depth,
        &PartitionSearchConfig::full(),
        0,
        0,
        None,
    )
}

/// Encode a superblock with recursive partition search using explicit config.
///
/// Tries PARTITION_NONE at the current size, then optionally tries HORZ, VERT,
/// extended partitions, 4:1 partitions, and SPLIT, picking lowest RD cost.
/// Config gates which partition types and intra modes are evaluated.
///
/// `recon` is the full frame (or standalone block) reconstruction buffer with
/// `recon_stride`; the block lives at (abs_x, abs_y). Predictions read
/// above/left neighbors directly from this buffer — including neighbors
/// inside the current superblock — exactly as the decoder reconstructs them.
/// When `ref_ctx` is provided, inter prediction is also tried using ME.
pub fn partition_search_with_config(
    src: &[u8],
    src_stride: usize,
    recon: &mut [u8],
    recon_stride: usize,
    width: usize,
    height: usize,
    qindex: u8,
    lambda: u64,
    max_depth: u32,
    config: &PartitionSearchConfig,
    abs_x: usize,
    abs_y: usize,
    ref_ctx: Option<&RefFrameCtx>,
) -> PartitionResult {
    // Base case: minimum size or max depth reached
    if width <= MIN_BLOCK_SIZE || height <= MIN_BLOCK_SIZE || max_depth == 0 {
        let mut leaf = encode_with_neighbors(
            src,
            src_stride,
            recon,
            recon_stride,
            width,
            height,
            qindex,
            config,
            abs_x,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::None,
            false,
        );
        add_none_node_cost(&mut leaf, width, height, lambda);
        return leaf;
    }

    // Try PARTITION_NONE: encode at current size
    let mut none_result = encode_with_neighbors(
        src,
        src_stride,
        recon,
        recon_stride,
        width,
        height,
        qindex,
        config,
        abs_x,
        abs_y,
        ref_ctx,
        svtav1_types::partition::PartitionType::None,
        false,
    );
    // Every square node the tile writer visits codes a partition symbol:
    // price PARTITION_NONE with its real entropy cost and rescore with
    // THIS search's lambda — the leaf rd formula uses a fixed scale, and
    // comparing that against the lambda-scaled candidates below is what
    // used to misprice NONE (docs/IDENTITY-STATUS.md, op-0 divergence).
    add_none_node_cost(&mut none_result, width, height, lambda);

    // If block is small enough, don't bother splitting further
    if width <= 8 && height <= 8 {
        return none_result;
    }

    let mut best_result = none_result;
    // Snapshot of the winning candidate's reconstruction for this region
    // (PARTITION_NONE was just encoded into the buffer).
    let mut best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);

    // Try PARTITION_HORZ: two halves stacked vertically.
    // Children are height/2 tall — gate keeps them >= min_block_dim
    // (identical to the historical `height >= 8` at min_block_dim = 4).
    if height >= 2 * config.min_block_dim {
        let hh = height / 2;
        let mut horz_result = PartitionResult {
            partition_type: PartitionType::Horz,
            rd_cost: 0,
            distortion: 0,
            rate: partition_rate_256(width, PartitionType::Horz),
            num_blocks: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
        };
        // Top half
        let top = encode_with_neighbors(
            src,
            src_stride,
            recon,
            recon_stride,
            width,
            hh,
            qindex,
            config,
            abs_x,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::Horz,
            false,
        );
        horz_result.distortion += top.distortion;
        horz_result.rate += top.rate;
        horz_result.num_blocks += top.num_blocks;
        horz_result.decisions.extend(top.decisions);

        // Bottom half — neighbors come straight from the live buffer.
        let bot = encode_with_neighbors(
            &src[hh * src_stride..],
            src_stride,
            recon,
            recon_stride,
            width,
            height - hh,
            qindex,
            config,
            abs_x,
            abs_y + hh,
            ref_ctx,
            svtav1_types::partition::PartitionType::Horz,
            false,
        );
        horz_result.distortion += bot.distortion;
        horz_result.rate += bot.rate;
        horz_result.num_blocks += bot.num_blocks;
        horz_result.decisions.extend(bot.decisions);
        let mut horz_children = alloc::vec::Vec::new();
        if let Some(t) = top.tree {
            horz_children.push(t);
        }
        if let Some(t) = bot.tree {
            horz_children.push(t);
        }
        horz_result.tree = Some(PartitionTree::Split {
            partition_type: PartitionType::Horz,
            width: width as u16,
            height: height as u16,
            children: horz_children,
        });
        horz_result.rd_cost = horz_result.distortion + ((lambda * horz_result.rate as u64) >> 8);

        if horz_result.rd_cost < best_result.rd_cost {
            best_result = horz_result;
            best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);
        }
    }

    // Try PARTITION_VERT: two halves side by side
    if width >= 2 * config.min_block_dim {
        let hw = width / 2;
        let mut vert_result = PartitionResult {
            partition_type: PartitionType::Vert,
            rd_cost: 0,
            distortion: 0,
            rate: partition_rate_256(width, PartitionType::Vert),
            num_blocks: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
        };
        // Left half
        let left = encode_with_neighbors(
            src,
            src_stride,
            recon,
            recon_stride,
            hw,
            height,
            qindex,
            config,
            abs_x,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::Vert,
            false,
        );
        vert_result.distortion += left.distortion;
        vert_result.rate += left.rate;
        vert_result.num_blocks += left.num_blocks;
        vert_result.decisions.extend(left.decisions);

        // Right half — neighbors come straight from the live buffer.
        let right = encode_with_neighbors(
            &src[hw..],
            src_stride,
            recon,
            recon_stride,
            width - hw,
            height,
            qindex,
            config,
            abs_x + hw,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::Vert,
            false,
        );
        vert_result.distortion += right.distortion;
        vert_result.rate += right.rate;
        vert_result.num_blocks += right.num_blocks;
        vert_result.decisions.extend(right.decisions);
        let mut vert_children = alloc::vec::Vec::new();
        if let Some(t) = left.tree {
            vert_children.push(t);
        }
        if let Some(t) = right.tree {
            vert_children.push(t);
        }
        vert_result.tree = Some(PartitionTree::Split {
            partition_type: PartitionType::Vert,
            width: width as u16,
            height: height as u16,
            children: vert_children,
        });
        vert_result.rd_cost = vert_result.distortion + ((lambda * vert_result.rate as u64) >> 8);

        if vert_result.rd_cost < best_result.rd_cost {
            best_result = vert_result;
            best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);
        }
    }

    // Try PARTITION_HORZ_4: four horizontal strips (each height/4)
    // Gated by config.enable_4to1_partitions (Spec 10: "4:1 partitions at preset <= 6")
    if height >= 4 * config.min_block_dim && config.enable_4to1_partitions {
        let qh = height / 4;
        let mut h4_result = PartitionResult {
            partition_type: PartitionType::Horz4,
            rd_cost: 0,
            distortion: 0,
            rate: partition_rate_256(width, PartitionType::Horz4),
            num_blocks: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
        };
        let mut h4_children = alloc::vec::Vec::new();
        for strip in 0..4 {
            let y0 = strip * qh;
            let cur_h = qh.min(height - y0);
            let sub = encode_with_neighbors(
                &src[y0 * src_stride..],
                src_stride,
                recon,
                recon_stride,
                width,
                cur_h,
                qindex,
                config,
                abs_x,
                abs_y + y0,
                ref_ctx,
                svtav1_types::partition::PartitionType::Horz4,
                false,
            );
            h4_result.distortion += sub.distortion;
            h4_result.rate += sub.rate;
            h4_result.num_blocks += sub.num_blocks;
            h4_result.decisions.extend(sub.decisions);
            if let Some(t) = sub.tree {
                h4_children.push(t);
            }
        }
        h4_result.tree = Some(PartitionTree::Split {
            partition_type: PartitionType::Horz4,
            width: width as u16,
            height: height as u16,
            children: h4_children,
        });
        h4_result.rd_cost = h4_result.distortion + ((lambda * h4_result.rate as u64) >> 8);
        if h4_result.rd_cost < best_result.rd_cost {
            best_result = h4_result;
            best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);
        }
    }

    // Try PARTITION_VERT_4: four vertical strips (each width/4)
    if width >= 4 * config.min_block_dim && config.enable_4to1_partitions {
        let qw = width / 4;
        let mut v4_result = PartitionResult {
            partition_type: PartitionType::Vert4,
            rd_cost: 0,
            distortion: 0,
            rate: partition_rate_256(width, PartitionType::Vert4),
            num_blocks: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
        };
        let mut v4_children = alloc::vec::Vec::new();
        for strip in 0..4 {
            let x0 = strip * qw;
            let cur_w = qw.min(width - x0);
            let sub = encode_with_neighbors(
                &src[x0..],
                src_stride,
                recon,
                recon_stride,
                cur_w,
                height,
                qindex,
                config,
                abs_x + x0,
                abs_y,
                ref_ctx,
                svtav1_types::partition::PartitionType::Vert4,
                false,
            );
            v4_result.distortion += sub.distortion;
            v4_result.rate += sub.rate;
            v4_result.num_blocks += sub.num_blocks;
            v4_result.decisions.extend(sub.decisions);
            if let Some(t) = sub.tree {
                v4_children.push(t);
            }
        }
        v4_result.tree = Some(PartitionTree::Split {
            partition_type: PartitionType::Vert4,
            width: width as u16,
            height: height as u16,
            children: v4_children,
        });
        v4_result.rd_cost = v4_result.distortion + ((lambda * v4_result.rate as u64) >> 8);
        if v4_result.rd_cost < best_result.rd_cost {
            best_result = v4_result;
            best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);
        }
    }

    // Try PARTITION_HORZ_A: top split into 2 quarters + bottom half
    // Gated by config.enable_ext_partitions (Spec 10: "extended partitions at preset <= 8")
    if width >= 2 * config.min_block_dim
        && height >= 2 * config.min_block_dim
        && config.enable_ext_partitions
    {
        let hw = width / 2;
        let hh = height / 2;
        let mut ha_result = PartitionResult {
            partition_type: PartitionType::HorzA,
            rd_cost: 0,
            distortion: 0,
            rate: partition_rate_256(width, PartitionType::HorzA),
            num_blocks: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
        };
        let mut ha_children = alloc::vec::Vec::new();
        // Top-left quarter
        let s = encode_with_neighbors(
            src,
            src_stride,
            recon,
            recon_stride,
            hw,
            hh,
            qindex,
            config,
            abs_x,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::HorzA,
            false,
        );
        ha_result.distortion += s.distortion;
        ha_result.rate += s.rate;
        ha_result.num_blocks += s.num_blocks;
        ha_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            ha_children.push(t);
        }
        // Top-right quarter
        let s = encode_with_neighbors(
            &src[hw..],
            src_stride,
            recon,
            recon_stride,
            width - hw,
            hh,
            qindex,
            config,
            abs_x + hw,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::HorzA,
            false,
        );
        ha_result.distortion += s.distortion;
        ha_result.rate += s.rate;
        ha_result.num_blocks += s.num_blocks;
        ha_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            ha_children.push(t);
        }
        // Bottom half
        let s = encode_with_neighbors(
            &src[hh * src_stride..],
            src_stride,
            recon,
            recon_stride,
            width,
            height - hh,
            qindex,
            config,
            abs_x,
            abs_y + hh,
            ref_ctx,
            svtav1_types::partition::PartitionType::HorzA,
            false,
        );
        ha_result.distortion += s.distortion;
        ha_result.rate += s.rate;
        ha_result.num_blocks += s.num_blocks;
        ha_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            ha_children.push(t);
        }
        ha_result.tree = Some(PartitionTree::Split {
            partition_type: PartitionType::HorzA,
            width: width as u16,
            height: height as u16,
            children: ha_children,
        });
        ha_result.rd_cost = ha_result.distortion + ((lambda * ha_result.rate as u64) >> 8);
        if ha_result.rd_cost < best_result.rd_cost {
            best_result = ha_result;
            best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);
        }
    }

    // Try PARTITION_HORZ_B: top half + bottom split into 2 quarters
    if width >= 2 * config.min_block_dim
        && height >= 2 * config.min_block_dim
        && config.enable_ext_partitions
    {
        let hw = width / 2;
        let hh = height / 2;
        let mut hb_result = PartitionResult {
            partition_type: PartitionType::HorzB,
            rd_cost: 0,
            distortion: 0,
            rate: partition_rate_256(width, PartitionType::HorzB),
            num_blocks: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
        };
        let mut hb_children = alloc::vec::Vec::new();
        // Top half
        let s = encode_with_neighbors(
            src,
            src_stride,
            recon,
            recon_stride,
            width,
            hh,
            qindex,
            config,
            abs_x,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::HorzB,
            false,
        );
        hb_result.distortion += s.distortion;
        hb_result.rate += s.rate;
        hb_result.num_blocks += s.num_blocks;
        hb_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            hb_children.push(t);
        }
        // Bottom-left quarter
        let s = encode_with_neighbors(
            &src[hh * src_stride..],
            src_stride,
            recon,
            recon_stride,
            hw,
            height - hh,
            qindex,
            config,
            abs_x,
            abs_y + hh,
            ref_ctx,
            svtav1_types::partition::PartitionType::HorzB,
            false,
        );
        hb_result.distortion += s.distortion;
        hb_result.rate += s.rate;
        hb_result.num_blocks += s.num_blocks;
        hb_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            hb_children.push(t);
        }
        // Bottom-right quarter
        let s = encode_with_neighbors(
            &src[hh * src_stride + hw..],
            src_stride,
            recon,
            recon_stride,
            width - hw,
            height - hh,
            qindex,
            config,
            abs_x + hw,
            abs_y + hh,
            ref_ctx,
            svtav1_types::partition::PartitionType::HorzB,
            false,
        );
        hb_result.distortion += s.distortion;
        hb_result.rate += s.rate;
        hb_result.num_blocks += s.num_blocks;
        hb_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            hb_children.push(t);
        }
        hb_result.tree = Some(PartitionTree::Split {
            partition_type: PartitionType::HorzB,
            width: width as u16,
            height: height as u16,
            children: hb_children,
        });
        hb_result.rd_cost = hb_result.distortion + ((lambda * hb_result.rate as u64) >> 8);
        if hb_result.rd_cost < best_result.rd_cost {
            best_result = hb_result;
            best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);
        }
    }

    // Try PARTITION_VERT_A: left split into 2 quarters + right half
    if width >= 2 * config.min_block_dim
        && height >= 2 * config.min_block_dim
        && config.enable_ext_partitions
    {
        let hw = width / 2;
        let hh = height / 2;
        let mut va_result = PartitionResult {
            partition_type: PartitionType::VertA,
            rd_cost: 0,
            distortion: 0,
            rate: partition_rate_256(width, PartitionType::VertA),
            num_blocks: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
        };
        let mut va_children = alloc::vec::Vec::new();
        // Top-left quarter
        let s = encode_with_neighbors(
            src,
            src_stride,
            recon,
            recon_stride,
            hw,
            hh,
            qindex,
            config,
            abs_x,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::VertA,
            false,
        );
        va_result.distortion += s.distortion;
        va_result.rate += s.rate;
        va_result.num_blocks += s.num_blocks;
        va_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            va_children.push(t);
        }
        // Bottom-left quarter
        let s = encode_with_neighbors(
            &src[hh * src_stride..],
            src_stride,
            recon,
            recon_stride,
            hw,
            height - hh,
            qindex,
            config,
            abs_x,
            abs_y + hh,
            ref_ctx,
            svtav1_types::partition::PartitionType::VertA,
            false,
        );
        va_result.distortion += s.distortion;
        va_result.rate += s.rate;
        va_result.num_blocks += s.num_blocks;
        va_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            va_children.push(t);
        }
        // Right half
        let s = encode_with_neighbors(
            &src[hw..],
            src_stride,
            recon,
            recon_stride,
            width - hw,
            height,
            qindex,
            config,
            abs_x + hw,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::VertA,
            false,
        );
        va_result.distortion += s.distortion;
        va_result.rate += s.rate;
        va_result.num_blocks += s.num_blocks;
        va_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            va_children.push(t);
        }
        va_result.tree = Some(PartitionTree::Split {
            partition_type: PartitionType::VertA,
            width: width as u16,
            height: height as u16,
            children: va_children,
        });
        va_result.rd_cost = va_result.distortion + ((lambda * va_result.rate as u64) >> 8);
        if va_result.rd_cost < best_result.rd_cost {
            best_result = va_result;
            best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);
        }
    }

    // Try PARTITION_VERT_B: left half + right split into 2 quarters
    if width >= 2 * config.min_block_dim
        && height >= 2 * config.min_block_dim
        && config.enable_ext_partitions
    {
        let hw = width / 2;
        let hh = height / 2;
        let mut vb_result = PartitionResult {
            partition_type: PartitionType::VertB,
            rd_cost: 0,
            distortion: 0,
            rate: partition_rate_256(width, PartitionType::VertB),
            num_blocks: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
        };
        let mut vb_children = alloc::vec::Vec::new();
        // Left half
        let s = encode_with_neighbors(
            src,
            src_stride,
            recon,
            recon_stride,
            hw,
            height,
            qindex,
            config,
            abs_x,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::VertB,
            false,
        );
        vb_result.distortion += s.distortion;
        vb_result.rate += s.rate;
        vb_result.num_blocks += s.num_blocks;
        vb_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            vb_children.push(t);
        }
        // Top-right quarter
        let s = encode_with_neighbors(
            &src[hw..],
            src_stride,
            recon,
            recon_stride,
            width - hw,
            hh,
            qindex,
            config,
            abs_x + hw,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::VertB,
            false,
        );
        vb_result.distortion += s.distortion;
        vb_result.rate += s.rate;
        vb_result.num_blocks += s.num_blocks;
        vb_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            vb_children.push(t);
        }
        // Bottom-right quarter
        let s = encode_with_neighbors(
            &src[hh * src_stride + hw..],
            src_stride,
            recon,
            recon_stride,
            width - hw,
            height - hh,
            qindex,
            config,
            abs_x + hw,
            abs_y + hh,
            ref_ctx,
            svtav1_types::partition::PartitionType::VertB,
            false,
        );
        vb_result.distortion += s.distortion;
        vb_result.rate += s.rate;
        vb_result.num_blocks += s.num_blocks;
        vb_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            vb_children.push(t);
        }
        vb_result.tree = Some(PartitionTree::Split {
            partition_type: PartitionType::VertB,
            width: width as u16,
            height: height as u16,
            children: vb_children,
        });
        vb_result.rd_cost = vb_result.distortion + ((lambda * vb_result.rate as u64) >> 8);
        if vb_result.rd_cost < best_result.rd_cost {
            best_result = vb_result;
            best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);
        }
    }

    // Try PARTITION_SPLIT: encode 4 sub-blocks.
    // With the default min_block_dim (4) SPLIT is tried unconditionally,
    // exactly as before; with the 4:2:0 min-8x8 policy it is gated so
    // quadrants never drop below min_block_dim. (The `width <= 8 &&
    // height <= 8` early-return above already prevents SPLIT below 16 for
    // the square blocks the recursion produces — this gate makes the
    // policy explicit for any caller-supplied shape.)
    let allow_split = config.min_block_dim <= MIN_BLOCK_SIZE
        || (width / 2 >= config.min_block_dim && height / 2 >= config.min_block_dim);
    if allow_split {
        let hw = width / 2;
        let hh = height / 2;
        let mut split_result = PartitionResult {
            partition_type: PartitionType::Split,
            rd_cost: 0,
            distortion: 0,
            rate: partition_rate_256(width, PartitionType::Split),
            num_blocks: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
        };

        // Encode 4 quadrants, collect child trees
        let mut split_children = alloc::vec::Vec::new();
        for (qr, qc) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            let x0 = qc * hw;
            let y0 = qr * hh;
            let cur_w = hw.min(width - x0);
            let cur_h = hh.min(height - y0);

            let sub_src_offset = y0 * src_stride + x0;

            let sub = partition_search_with_config(
                &src[sub_src_offset..],
                src_stride,
                recon,
                recon_stride,
                cur_w,
                cur_h,
                qindex,
                lambda,
                max_depth - 1,
                config,
                abs_x + x0,
                abs_y + y0,
                ref_ctx,
            );

            split_result.distortion += sub.distortion;
            split_result.rate += sub.rate;
            split_result.num_blocks += sub.num_blocks;
            split_result.decisions.extend(sub.decisions);
            if let Some(t) = sub.tree {
                split_children.push(t);
            }
        }
        split_result.tree = Some(PartitionTree::Split {
            partition_type: PartitionType::Split,
            width: width as u16,
            height: height as u16,
            children: split_children,
        });
        split_result.rd_cost = split_result.distortion + ((lambda * split_result.rate as u64) >> 8);

        // Check if SPLIT is better than current best
        if split_result.rd_cost < best_result.rd_cost {
            best_result = split_result;
            best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);
        }
    }

    // Leave the winning candidate's reconstruction in the buffer.
    restore_region(recon, recon_stride, abs_x, abs_y, width, height, &best_snap);
    best_result
}

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
    let (qcoeffs, eob, tx_type) = if choice.tx_depth == 0 {
        // Unpack the packed (<= 32-capped) txb into the full
        // w x h raster the depth-0 walk path expects.
        let (pw, ph) = (w.min(32), h.min(32));
        let mut full = alloc::vec![0i32; w * h];
        for r in 0..ph {
            for c in 0..pw {
                full[r * w + c] = choice.txb_qcoeffs[0][r * pw + c];
            }
        }
        let tx_type = choice.txb_tx_types[0];
        // AV1 eob is a SCAN-ORDER quantity. The previous raster-order
        // last-nonzero was only ever consumed as `== 0` (correct either way),
        // but it made the SVTAV1_DUMP_TREE `eob` field wildly misleading — a
        // 32x32 leaf whose true scan-order eob is 299 showed as ~706 (the
        // raster index of the last retained diagonal coeff). Compute the real
        // scan-order eob from the packed txb with the coder's scan so the
        // dump matches the bitstream (the coder re-derives it identically at
        // pipeline.rs `write_coeffs_txb_1d`).
        let tx_size = svtav1_entropy::coeff_c::tx_size_from_dims(pw, ph);
        let sidx =
            svtav1_entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[tx_type as usize] as usize;
        let scan = svtav1_entropy::scan_tables::scan(tx_size, sidx);
        let mut eob = 0u16;
        for (i, &pos) in scan.iter().enumerate() {
            if choice.txb_qcoeffs[0][pos as usize] != 0 {
                eob = (i + 1) as u16;
            }
        }
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
        ..Default::default()
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn encode_fixed_tree(
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
) -> PartitionResult {
    match tree {
        crate::pd0::Pd0Tree::Leaf(leaf_size) => {
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
                        0,
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
                        children: alloc::vec![PartitionTree::Leaf(decision.clone())],
                    };
                    return PartitionResult {
                        partition_type: ptype,
                        rd_cost: 0,
                        distortion: 0,
                        rate: 0,
                        decisions: alloc::vec![decision],
                        tree: Some(tree),
                        num_blocks: 1,
                    };
                }
                let choice = crate::leaf_funnel::decide_leaf(
                    fx,
                    src,
                    src_stride,
                    0,
                    recon,
                    recon_stride,
                    abs_x,
                    abs_y,
                    size,
                    dc_only,
                    sb_is_lvl6,
                );
                let decision = funnel_block_decision(choice, size, size);
                let tree = PartitionTree::Leaf(decision.clone());
                return PartitionResult {
                    partition_type: PartitionType::None,
                    rd_cost: 0,
                    distortion: 0,
                    rate: 0,
                    decisions: alloc::vec![decision],
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
            let dc_only =
                crate::pd0::is_dc_only_safe(sb_vars, size, abs_x - sb_org.0, abs_y - sb_org.1);
            encode_with_neighbors(
                src,
                src_stride,
                recon,
                recon_stride,
                size,
                size,
                qindex,
                config,
                abs_x,
                abs_y,
                None,
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
                    &src[y0 * src_stride + x0..],
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

/// Encode one chroma plane's block for the 4:2:0 path: UV_DC prediction
/// from the live chroma reconstruction plane, full-block DCT-DCT transform
/// and quantization at the SAME qindex tables as luma (the frame header
/// signals DeltaQUDc = DeltaQUAc = 0, so the decoder dequantizes chroma
/// with the identical step sizes), reconstructing into the plane.
///
/// `src`/`recon` are full (w/2 x h/2) chroma planes with `stride`; the
/// block lives at chroma coords (cx, cy) with chroma dims (cw, ch).
/// Neighbor extraction reuses the C-exact edge fill (127/129/left[0]/
/// above[0] rules) on the chroma plane; DC prediction and the
/// transform/quant/recon cycle are the same decoder-mirrored paths the
/// luma side uses. Must be called in coding order — the prediction reads
/// previously reconstructed chroma neighbors exactly as the decoder will.
///
/// Returns (qcoeffs raster cw x ch, eob) for the entropy writer.
///
/// `cq`: the frame-level C-exact coding quantizer (still path). C's MDS3
/// runs RDOQ on chroma too when enc-dec is bypassed (`md_stage_3` clears
/// `rdoq_ctrls.skip_uv`, product_coding_loop.c) — plane_type 1 selects the
/// chroma cost tables and `plane_rd_mult` 13.
/// `tile_top` is the CHROMA-plane pixel row where the current tile
/// starts (0 = single tile row). Callers pass the luma tile-row origin
/// halved — exact since tile rows are always 64-luma-px (SB-aligned)
/// multiples, and 4:2:0 chroma is exactly half resolution vertically.
#[allow(clippy::too_many_arguments)]
pub fn encode_chroma_block_dc(
    src: &[u8],
    recon: &mut [u8],
    stride: usize,
    cx: usize,
    cy: usize,
    cw: usize,
    ch: usize,
    qindex: u8,
    cq: Option<&crate::quant::CodingQuantCfg>,
    qm_level: u8,
    tile_top: usize,
) -> (alloc::vec::Vec<i32>, u16) {
    let (above, left, _top_left, has_above, has_left) =
        extract_neighbors_tiled(recon, stride, cx, cy, cw, ch, tile_top);

    let mut pred = alloc::vec![0u8; cw * ch];
    svtav1_dsp::intra_pred::predict_dc(&mut pred, cw, &above, &left, cw, ch, has_above, has_left);

    let enc = crate::encode_loop::encode_block_tx_cq(
        &src[cy * stride + cx..],
        stride,
        &pred,
        cw,
        cw,
        ch,
        qindex,
        svtav1_types::transform::TxType::DctDct,
        cq,
        1,
        qm_level,
    );

    for r in 0..ch {
        let dst = (cy + r) * stride + cx;
        recon[dst..dst + cw].copy_from_slice(&enc.recon[r * cw..r * cw + cw]);
    }

    (enc.qcoeffs, enc.eob)
}

/// Helper: extract neighbors from frame context and encode a single block.
///
/// `partition` is the partition type this block will be SIGNALED under
/// (the parent candidate's type; None for PARTITION_NONE leaves). It must
/// be the real one: the decoder selects different has_top_right /
/// has_bottom_left availability tables for PARTITION_VERT_A/VERT_B
/// children (libaom get_has_tr_table / get_has_bl_table), and coding a
/// directional block against the wrong availability makes the encoder
/// extend edges with pixels the decoder never sees.
///
/// `dc_only` restricts the intra candidate set to exactly {DC_PRED} — the
/// C `is_dc_only_safe` outcome on the still/PD1 fixed-tree path (C injects
/// no other candidate, so no cost compare runs; mode_decision.c:3633).
#[allow(clippy::too_many_arguments)]
fn encode_with_neighbors(
    src: &[u8],
    src_stride: usize,
    recon: &mut [u8],
    recon_stride: usize,
    width: usize,
    height: usize,
    qindex: u8,
    config: &PartitionSearchConfig,
    abs_x: usize,
    abs_y: usize,
    ref_ctx: Option<&RefFrameCtx>,
    partition: svtav1_types::partition::PartitionType,
    dc_only: bool,
) -> PartitionResult {
    let (above, left, top_left, has_above, has_left) =
        extract_neighbors_tiled(recon, recon_stride, abs_x, abs_y, width, height, config.tile_top_px);
    encode_single_block(
        src,
        src_stride,
        recon,
        recon_stride,
        width,
        height,
        qindex,
        config,
        &above,
        &left,
        top_left,
        has_above,
        has_left,
        ref_ctx,
        abs_x,
        abs_y,
        partition,
        dc_only,
    )
}

/// Encode a single block with mode decision — tries multiple intra
/// prediction modes and picks the one with lowest RD cost.
/// When `ref_ctx` is provided, also tries inter prediction using ME.
///
/// Generate an inter prediction block from reference + MV with bilinear interpolation.
/// Supports full-pel, half-pel, and quarter-pel positions.
fn generate_inter_pred(
    rfc: &RefFrameCtx,
    mv: svtav1_types::motion::Mv,
    abs_x: usize,
    abs_y: usize,
    width: usize,
    height: usize,
) -> alloc::vec::Vec<u8> {
    let int_x = abs_x as i32 + (mv.x as i32 >> 3);
    let int_y = abs_y as i32 + (mv.y as i32 >> 3);
    let fx = (mv.x & 7) as i32;
    let fy = (mv.y & 7) as i32;
    let n = width * height;
    let mut pred = alloc::vec![128u8; n];
    for r in 0..height {
        for c in 0..width {
            let ry = int_y + r as i32;
            let rx = int_x + c as i32;
            if ry >= 0
                && (ry as usize + 1) < rfc.pic_height
                && rx >= 0
                && (rx as usize + 1) < rfc.pic_width
            {
                let off = ry as usize * rfc.stride + rx as usize;
                let val = if fx == 0 && fy == 0 {
                    rfc.y_plane[off] as i32
                } else if fy == 0 {
                    ((8 - fx) * rfc.y_plane[off] as i32 + fx * rfc.y_plane[off + 1] as i32 + 4) >> 3
                } else if fx == 0 {
                    ((8 - fy) * rfc.y_plane[off] as i32
                        + fy * rfc.y_plane[off + rfc.stride] as i32
                        + 4)
                        >> 3
                } else {
                    let tl = rfc.y_plane[off] as i32;
                    let tr = rfc.y_plane[off + 1] as i32;
                    let bl = rfc.y_plane[off + rfc.stride] as i32;
                    let br = rfc.y_plane[off + rfc.stride + 1] as i32;
                    let top = (8 - fx) * tl + fx * tr;
                    let bot = (8 - fx) * bl + fx * br;
                    ((8 - fy) * top + fy * bot + 32) >> 6
                };
                pred[r * width + c] = val.clamp(0, 255) as u8;
            }
        }
    }
    pred
}

/// Uses the provided `above`/`left`/`top_left` neighbor arrays for prediction.
/// `has_above`/`has_left` control DC prediction averaging (false at frame edges).
/// (Spec 05, Section 7.11.2)
fn encode_single_block(
    src: &[u8],
    src_stride: usize,
    recon: &mut [u8],
    recon_stride: usize,
    width: usize,
    height: usize,
    qindex: u8,
    config: &PartitionSearchConfig,
    above: &[u8],
    left: &[u8],
    top_left: u8,
    has_above: bool,
    has_left: bool,
    ref_ctx: Option<&RefFrameCtx>,
    abs_x: usize,
    abs_y: usize,
    partition: svtav1_types::partition::PartitionType,
    dc_only: bool,
) -> PartitionResult {
    let n = width * height;
    // Mode-RD lambda: CLI-qp-calibrated closed form via the exact inverse
    // mapping (see qp_to_lambda's domain note — feeding the raw qindex
    // would scale lambda by ~2^48 and make rate dominate every decision).
    // Pre-existing wrinkle kept as-is: this recomputes an UNSCALED lambda
    // while the partition-level RD uses the speed-scaled one.
    let lambda =
        crate::rate_control::qp_to_lambda(crate::rate_control::qindex_to_qp(qindex)) as u64;

    // Try multiple intra modes via mode decision.
    // Number of candidates controlled by block size and spec 03 NIC rules.
    let block_size = if width >= 8 && height >= 8 {
        svtav1_types::block::BlockSize::Block8x8
    } else {
        svtav1_types::block::BlockSize::Block4x4
    };
    let all_candidates = crate::mode_decision::generate_intra_candidates(block_size);
    // Limit candidates per config.max_intra_candidates (spec 03: NIC)
    let max_cands = config
        .max_intra_candidates
        .min(if width <= 4 || height <= 4 { 3 } else { 13 });
    // dc_only = the C is_dc_only_safe gate fired: the candidate set is
    // exactly {DC_PRED} (generate_intra_candidates puts DC first), like
    // C's inject_intra_candidates with dc_cand_only_flag.
    let candidates = if dc_only {
        &all_candidates[..1]
    } else {
        &all_candidates[..max_cands.min(all_candidates.len())]
    };

    let mut best_enc = None;
    let mut best_cost = u64::MAX;
    let mut chose_inter = false;
    let mut chosen_mv = svtav1_types::motion::Mv::ZERO;
    // AV1 y_mode index of the winning intra candidate — MUST match what the
    // bitstream signals, or the decoder predicts with a different mode than
    // the one the residual was built against.
    let mut chosen_mode: u8 = 0;
    let mut chosen_tx: u8 = 0; // C TxType index (DCT_DCT = 0)

    for cand in candidates {
        let mut pred_block = alloc::vec![128u8; n];

        // Generate prediction for this mode
        match cand.mode {
            svtav1_types::prediction::PredictionMode::DcPred => {
                svtav1_dsp::intra_pred::predict_dc(
                    &mut pred_block,
                    width,
                    above,
                    left,
                    width,
                    height,
                    has_above,
                    has_left,
                );
            }
            svtav1_types::prediction::PredictionMode::VPred => {
                svtav1_dsp::intra_pred::predict_v(&mut pred_block, width, above, width, height);
            }
            svtav1_types::prediction::PredictionMode::HPred => {
                svtav1_dsp::intra_pred::predict_h(&mut pred_block, width, left, width, height);
            }
            svtav1_types::prediction::PredictionMode::SmoothPred => {
                svtav1_dsp::intra_pred::predict_smooth(
                    &mut pred_block,
                    width,
                    above,
                    left,
                    width,
                    height,
                );
            }
            svtav1_types::prediction::PredictionMode::PaethPred => {
                svtav1_dsp::intra_pred::predict_paeth(
                    &mut pred_block,
                    width,
                    above,
                    left,
                    top_left,
                    width,
                    height,
                );
            }
            svtav1_types::prediction::PredictionMode::SmoothVPred => {
                svtav1_dsp::intra_pred::predict_smooth_v(
                    &mut pred_block,
                    width,
                    above,
                    left,
                    0,
                    height,
                    width,
                );
            }
            svtav1_types::prediction::PredictionMode::SmoothHPred => {
                svtav1_dsp::intra_pred::predict_smooth_h(
                    &mut pred_block,
                    width,
                    above,
                    left,
                    width,
                    height,
                );
            }
            svtav1_types::prediction::PredictionMode::D45Pred
            | svtav1_types::prediction::PredictionMode::D67Pred
            | svtav1_types::prediction::PredictionMode::D135Pred
            | svtav1_types::prediction::PredictionMode::D113Pred
            | svtav1_types::prediction::PredictionMode::D157Pred
            | svtav1_types::prediction::PredictionMode::D203Pred => {
                let angle = match cand.mode {
                    svtav1_types::prediction::PredictionMode::D45Pred => 45,
                    svtav1_types::prediction::PredictionMode::D67Pred => 67,
                    svtav1_types::prediction::PredictionMode::D113Pred => 113,
                    svtav1_types::prediction::PredictionMode::D135Pred => 135,
                    svtav1_types::prediction::PredictionMode::D157Pred => 157,
                    svtav1_types::prediction::PredictionMode::D203Pred => 203,
                    _ => 45,
                };
                // Build the extended neighbor arrays exactly like the
                // decoder (libaom build_intra_predictors): real
                // above-right / bottom-left pixels where
                // has_top_right/has_bottom_left say they are decoded,
                // replication of the last real sample otherwise, and the
                // decoder's unavailable-edge fills — instead of the old
                // flat-128 padding the decoder never sees.
                //
                // `partition` is the type this block is SIGNALED under:
                // PARTITION_VERT_A/B children select the has_tr_vert_*/
                // has_bl_vert_* availability tables in the decoder
                // (libaom get_has_tr_table), everything else the generic
                // ones. The search DOES emit VERT_A/B (ext partitions,
                // preset <= 8) — passing None here coded VERT_A/B
                // D-mode children against above-right/bottom-left pixels
                // the decoder never has (recon-parity failures at
                // qindex >= 80 where ext partitions start winning).
                match crate::intra_edge::build_directional_edges(
                    recon,
                    recon_stride,
                    abs_x,
                    abs_y,
                    width,
                    height,
                    angle,
                    partition,
                    config.sb_mi_size,
                ) {
                    crate::intra_edge::DirEdges::Flat(v) => pred_block.fill(v),
                    crate::intra_edge::DirEdges::Edges {
                        above: ext_above,
                        left: ext_left,
                        top_left: ext_top_left,
                    } => {
                        svtav1_dsp::intra_pred::predict_directional(
                            &mut pred_block,
                            width,
                            &ext_above,
                            &ext_left,
                            ext_top_left,
                            width,
                            height,
                            angle,
                        );
                    }
                }
            }
            _ => {
                // Remaining directional modes and advanced modes — use DC as fallback
                svtav1_dsp::intra_pred::predict_dc(
                    &mut pred_block,
                    width,
                    above,
                    left,
                    width,
                    height,
                    has_above,
                    has_left,
                );
            }
        }

        // Encode with this prediction — try DCT-DCT first
        let enc_dct = crate::encode_loop::encode_block_tx_cq(
            src,
            src_stride,
            &pred_block,
            width,
            width,
            height,
            qindex,
            svtav1_types::transform::TxType::DctDct,
            config.c_quant.as_deref(),
            0,
            config.c_quant.as_deref().map_or(15, |c| c.qm_levels[0]),
        );
        let cost_dct = enc_dct.distortion + ((lambda * enc_dct.rate as u64) >> 8);

        if cost_dct < best_cost {
            best_cost = cost_dct;
            best_enc = Some(enc_dct);
            chosen_mode = cand.mode as u8;
            chosen_tx = svtav1_types::transform::TxType::DctDct as u8;
        }

        // RDO transform type selection for non-DC modes at sizes <= 16.
        // Gated by rdo_tx_decision (Spec 03: only at low presets) and
        // enable_adst (Spec 04: "ADST captures asymmetric energy").
        if config.rdo_tx_decision
            && config.enable_adst
            && width <= 16
            && height <= 16
            && cand.mode.is_intra()
        {
            // Select candidate TX types based on prediction mode
            let tx_candidates: &[svtav1_types::transform::TxType] = match cand.mode {
                svtav1_types::prediction::PredictionMode::VPred
                | svtav1_types::prediction::PredictionMode::D67Pred => {
                    // Vertical: ADST in column, DCT in row
                    &[svtav1_types::transform::TxType::AdstDct]
                }
                svtav1_types::prediction::PredictionMode::HPred
                | svtav1_types::prediction::PredictionMode::D203Pred => {
                    // Horizontal: DCT in column, ADST in row
                    &[svtav1_types::transform::TxType::DctAdst]
                }
                svtav1_types::prediction::PredictionMode::D45Pred
                | svtav1_types::prediction::PredictionMode::D135Pred => {
                    // Diagonal: ADST-ADST
                    &[svtav1_types::transform::TxType::AdstAdst]
                }
                svtav1_types::prediction::PredictionMode::PaethPred => {
                    // Paeth: try ADST-DCT
                    &[svtav1_types::transform::TxType::AdstDct]
                }
                _ => &[], // DC and smooth: DCT-DCT is optimal
            };

            for &alt_tx in tx_candidates {
                let enc_alt = crate::encode_loop::encode_block_tx_cq(
                    src,
                    src_stride,
                    &pred_block,
                    width,
                    width,
                    height,
                    qindex,
                    alt_tx,
                    config.c_quant.as_deref(),
                    0,
                    config.c_quant.as_deref().map_or(15, |c| c.qm_levels[0]),
                );
                let cost_alt = enc_alt.distortion + ((lambda * enc_alt.rate as u64) >> 8);
                if cost_alt < best_cost {
                    best_cost = cost_alt;
                    best_enc = Some(enc_alt);
                    chosen_mode = cand.mode as u8;
                    chosen_tx = alt_tx as u8;
                }
            }
        }
    }

    // Filter-intra candidates are NOT evaluated: the sequence header
    // signals enable_filter_intra = 0, so the bitstream cannot represent
    // them — using their prediction would diverge from the decoder.

    // Try inter prediction if a reference frame is available.
    // Runs hierarchical ME (full-pel + half-pel refinement) to find the best MV,
    // generates a bilinear-interpolated prediction, and compares RD cost.
    if let Some(rfc) = ref_ctx {
        let me_params = crate::motion_est::MeSearchParams {
            search_area_width: 16,
            search_area_height: 16,
            use_hme: false,
            subpel_level: 2, // half-pel + quarter-pel refinement
        };
        // Use spatial MV predictor from neighboring blocks as search center
        let center_mv = rfc.get_mv_predictor(abs_x, abs_y);
        let me_result = crate::motion_est::hierarchical_me_centered(
            src,
            src_stride,
            rfc.y_plane,
            rfc.stride,
            abs_x as i32,
            abs_y as i32,
            width,
            height,
            &me_params,
            rfc.pic_width,
            rfc.pic_height,
            center_mv,
        );

        // Generate inter prediction from reference + MV
        let mut inter_pred = generate_inter_pred(rfc, me_result.mv, abs_x, abs_y, width, height);

        // Apply OBMC blending with above/left neighbor predictions.
        // Uses neighbor MVs from the MV map to generate overlap predictions.
        // (Spec 06: OBMC blends current prediction with neighbor predictions)
        if let Some(mv_map) = rfc.mv_map {
            let bx = abs_x / 8;
            let by = abs_y / 8;
            let stride = rfc.mv_map_stride;
            let overlap_h = (height / 2).clamp(1, 4);
            let overlap_w = (width / 2).clamp(1, 4);

            // Above neighbor OBMC
            if by > 0 && stride > 0 {
                let above_mv = mv_map[(by - 1) * stride + bx];
                if above_mv != me_result.mv {
                    let above_pred =
                        generate_inter_pred(rfc, above_mv, abs_x, abs_y, width, overlap_h);
                    svtav1_dsp::obmc::obmc_blend_above(
                        &mut inter_pred,
                        width,
                        &above_pred,
                        width,
                        width,
                        height,
                        overlap_h,
                    );
                }
            }

            // Left neighbor OBMC
            if bx > 0 && stride > 0 {
                let left_mv = mv_map[by * stride + bx - 1];
                if left_mv != me_result.mv {
                    let left_pred =
                        generate_inter_pred(rfc, left_mv, abs_x, abs_y, overlap_w, height);
                    svtav1_dsp::obmc::obmc_blend_left(
                        &mut inter_pred,
                        width,
                        &left_pred,
                        overlap_w,
                        width,
                        height,
                        overlap_w,
                    );
                }
            }
        }

        let enc_inter = crate::encode_loop::encode_block(
            src,
            src_stride,
            &inter_pred,
            width,
            width,
            height,
            qindex,
        );
        // Add MV rate overhead (~2 bytes for simple MVs)
        let mv_rate = if me_result.mv.x == 0 && me_result.mv.y == 0 {
            64 // zero MV: ~0.25 bits
        } else {
            256 // nonzero MV: ~1 bit for joint + magnitude
        };
        let inter_cost = enc_inter.distortion + ((lambda * (enc_inter.rate + mv_rate) as u64) >> 8);
        if inter_cost < best_cost {
            best_enc = Some(enc_inter);
            chose_inter = true;
            chosen_mv = me_result.mv;
        }
    }

    let enc = best_enc.unwrap_or_else(|| {
        let pred_block = alloc::vec![128u8; n];
        crate::encode_loop::encode_block_tx_cq(
            src,
            src_stride,
            &pred_block,
            width,
            width,
            height,
            qindex,
            svtav1_types::transform::TxType::DctDct,
            config.c_quant.as_deref(),
            0,
            config.c_quant.as_deref().map_or(15, |c| c.qm_levels[0]),
        )
    });

    for r in 0..height {
        let dst = (abs_y + r) * recon_stride + abs_x;
        recon[dst..dst + width].copy_from_slice(&enc.recon[r * width..r * width + width]);
    }

    let decision = BlockDecision {
        partition_type: PartitionType::None,
        is_inter: chose_inter,
        intra_mode: chosen_mode,
        tx_type: if chose_inter { 0 } else { chosen_tx },
        mv: chosen_mv,
        qcoeffs: enc.qcoeffs.to_vec(),
        eob: enc.eob,
        width: width as u16,
        height: height as u16,
        ..Default::default()
    };

    let tree = PartitionTree::Leaf(decision.clone());

    PartitionResult {
        partition_type: PartitionType::None,
        rd_cost: enc.distortion + ((enc.rate as u64) << 4),
        distortion: enc.distortion,
        rate: enc.rate,
        decisions: alloc::vec![decision],
        tree: Some(tree),
        num_blocks: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn partition_search_uniform() {
        let src = vec![128u8; 16 * 16];
        let mut recon = vec![0u8; 16 * 16];
        let result = partition_search(&src, 16, &mut recon, 16, 16, 16, 30, 256, 3);
        assert_eq!(
            result.distortion, 0,
            "uniform block should have zero distortion"
        );
    }

    #[test]
    fn partition_search_gradient() {
        let mut src = vec![0u8; 32 * 32];
        for r in 0..32 {
            for c in 0..32 {
                src[r * 32 + c] = (r * 8 + c * 4) as u8;
            }
        }
        let mut recon = vec![0u8; 32 * 32];
        let result = partition_search(&src, 32, &mut recon, 32, 32, 32, 25, 256, 3);
        assert!(result.num_blocks > 1, "gradient should trigger splitting");
    }

    #[test]
    fn partition_respects_min_size() {
        let src = vec![100u8; 4 * 4];
        let mut recon = vec![0u8; 4 * 4];
        let result = partition_search(&src, 4, &mut recon, 4, 4, 4, 30, 256, 10);
        assert_eq!(result.num_blocks, 1, "4x4 should not split");
    }

    #[test]
    fn partition_search_produces_recon() {
        let src: Vec<u8> = (0..256).map(|i| (i % 256) as u8).collect();
        let mut recon = vec![0u8; 16 * 16];
        let result = partition_search(&src, 16, &mut recon, 16, 16, 16, 25, 256, 2);
        // Recon should be populated (not all zeros)
        assert!(recon.iter().any(|&v| v != 0), "recon should be non-zero");
        assert!(result.rd_cost > 0);
    }

    #[test]
    fn partition_search_reads_frame_neighbors() {
        // Frame buffer with a gradient row above the target block: the
        // search must read it as the above neighbors and reconstruct into
        // the same buffer.
        let w = 32usize;
        let h = 48usize;
        let mut frame = vec![128u8; w * h];
        for c in 0..w {
            frame[15 * w + c] = (c * 8) as u8; // row 15 = above the block at y=16
        }
        let mut src = vec![0u8; 16 * 16];
        for r in 0..16 {
            for c in 0..16 {
                src[r * 16 + c] = (c * 8) as u8;
            }
        }
        let result = partition_search_with_config(
            &src,
            16,
            &mut frame,
            w,
            16,
            16,
            30,
            256,
            2,
            &PartitionSearchConfig::full(),
            0,
            16,
            None,
        );
        assert!(result.num_blocks >= 1);
        // The block region must have been reconstructed (roughly matching src).
        let recon_center = frame[(16 + 8) * w + 8];
        assert!(
            (recon_center as i32 - src[8 * 16 + 8] as i32).abs() < 64,
            "recon {} vs src {}",
            recon_center,
            src[8 * 16 + 8]
        );
    }

    #[test]
    fn extract_neighbors_frame_edge() {
        // Both edges unavailable: the C decoder fills above with base-1 = 127
        // and left with base+1 = 129 (libaom reconintra.c), top-left 128.
        let frame = vec![100u8; 64 * 64];
        let (above, left, tl, has_above, has_left) = extract_neighbors(&frame, 64, 0, 0, 8, 8);
        assert!(!has_above);
        assert!(!has_left);
        assert!(above.iter().all(|&v| v == 127), "above fill: {above:?}");
        assert!(left.iter().all(|&v| v == 129), "left fill: {left:?}");
        assert_eq!(tl, 128);
    }

    #[test]
    fn extract_neighbors_single_edge_fill_matches_c() {
        // Above missing + left available: above[] = left_ref[0].
        // Left missing + above available: left[] = above_ref[0].
        let w = 64;
        let mut frame = vec![0u8; w * w];
        for r in 0..w {
            for c in 0..w {
                frame[r * w + c] = if c < 8 { 32 } else { 224 };
            }
        }
        // Block at (8, 0): top frame edge, left column available (value 32).
        let (above, left, tl, has_above, has_left) = extract_neighbors(&frame, w, 8, 0, 8, 8);
        assert!(!has_above);
        assert!(has_left);
        assert!(
            above.iter().all(|&v| v == 32),
            "above = left_ref[0]: {above:?}"
        );
        assert!(left.iter().all(|&v| v == 32));
        assert_eq!(tl, 32, "top-left = left_ref[0] when only left exists");

        // Block at (0, 8): left frame edge, above row available (value 32).
        let (above, left, tl, has_above, has_left) = extract_neighbors(&frame, w, 0, 8, 8, 8);
        assert!(has_above);
        assert!(!has_left);
        assert!(above.iter().all(|&v| v == 32));
        assert!(
            left.iter().all(|&v| v == 32),
            "left = above_ref[0]: {left:?}"
        );
        assert_eq!(tl, 32, "top-left = above_ref[0] when only above exists");
    }

    #[test]
    fn extract_neighbors_reads_above_row() {
        let w = 128;
        let h = 128;
        let mut frame = vec![0u8; w * h];
        for r in 0..64 {
            for c in 0..w {
                frame[r * w + c] = ((r + c) % 256) as u8;
            }
        }
        let (above, _left, _tl, has_above, has_left) = extract_neighbors(&frame, w, 0, 64, 8, 8);
        assert!(has_above);
        assert!(!has_left);
        for i in 0..8 {
            assert_eq!(above[i], ((63 + i) % 256) as u8);
        }
    }

    /// Task #86: the SAME position as `extract_neighbors_reads_above_row`
    /// (abs_y=64, real non-zero data sits in row 63) but with `tile_top =
    /// 64` — i.e. row 64 is THIS tile's own top row. `has_above` must be
    /// false (AV1 intra prediction never crosses a tile boundary) even
    /// though row 63 holds real, readable pixel data in the buffer — a
    /// conforming decoder has no such row for this tile and would desync
    /// if the encoder predicted from it.
    #[test]
    fn extract_neighbors_tiled_top_row_has_no_above() {
        let w = 128;
        let h = 128;
        let mut frame = vec![0u8; w * h];
        for r in 0..64 {
            for c in 0..w {
                frame[r * w + c] = ((r + c) % 256) as u8;
            }
        }
        let (above, left, tl, has_above, has_left) =
            extract_neighbors_tiled(&frame, w, 0, 64, 8, 8, 64);
        assert!(!has_above, "row 64 IS this tile's own top row");
        assert!(!has_left);
        // Unavailable-above fallback: left_ref[0] if left exists, else 127
        // (left is also unavailable here, abs_x=0) — matches
        // extract_neighbors_frame_edge's plain frame-edge expectation.
        assert!(above.iter().all(|&v| v == 127), "above = {above:?}");
        assert!(
            left.iter().all(|&v| v == 129),
            "left = above_ref[0].unwrap_or(129) when neither is available: {left:?}"
        );
        assert_eq!(tl, 128, "top-left = 128 when neither is available");
    }

    /// Same tile boundary, but abs_x > 0 so "left" IS available — the
    /// unavailable-above fallback must copy left_ref[0], not a flat 127.
    #[test]
    fn extract_neighbors_tiled_top_row_falls_back_to_left() {
        let w = 128;
        let mut frame = vec![0u8; w * 128];
        // Row 64 (this tile's own top row), starting at col 4: give the
        // "left" column (col 3) a distinct, non-127/128/129 value so the
        // fallback is unambiguous.
        frame[64 * w + 3] = 200;
        let (above, _left, tl, has_above, has_left) =
            extract_neighbors_tiled(&frame, w, 4, 64, 8, 8, 64);
        assert!(!has_above);
        assert!(has_left);
        assert!(
            above.iter().all(|&v| v == 200),
            "above = left_ref[0] when only left exists: {above:?}"
        );
        assert_eq!(tl, 200, "top-left = left_ref[0] when only left exists");
    }

    /// A block strictly BELOW a tile's top row (not the first row) still
    /// sees a real above neighbor from earlier in the SAME tile — only
    /// the tile's OWN top row loses availability.
    #[test]
    fn extract_neighbors_tiled_interior_row_has_above() {
        let w = 128;
        let mut frame = vec![0u8; w * 128];
        for c in 0..w {
            frame[71 * w + c] = 77;
        }
        let (above, _left, _tl, has_above, _has_left) =
            extract_neighbors_tiled(&frame, w, 0, 72, 8, 8, 64);
        assert!(has_above, "row 72 is inside the tile (top row = 64)");
        assert!(above.iter().all(|&v| v == 77));
    }

    #[test]
    fn extract_neighbors_reads_left_column() {
        let w = 128;
        let h = 64;
        let mut frame = vec![0u8; w * h];
        for r in 0..h {
            for c in 0..64 {
                frame[r * w + c] = ((r * 2 + c) % 256) as u8;
            }
        }
        let (_above, left, _tl, _has_above, has_left) = extract_neighbors(&frame, w, 64, 0, 8, 8);
        assert!(has_left);
        for i in 0..8 {
            assert_eq!(left[i], ((i * 2 + 63) % 256) as u8);
        }
    }

    #[test]
    fn extract_neighbors_in_sb_positions_are_live() {
        // Neighbors INSIDE the current superblock must be read from the
        // buffer (the historical bug returned 128 for them).
        let w = 64;
        let mut frame = vec![0u8; w * w];
        for c in 0..w {
            frame[7 * w + c] = 200; // row 7 — above a block at y=8 inside the SB
        }
        let (above, _left, _tl, has_above, _has_left) = extract_neighbors(&frame, w, 8, 8, 8, 8);
        assert!(has_above);
        assert!(
            above.iter().all(|&v| v == 200),
            "in-SB above must be live: {above:?}"
        );
    }
}

/// The per-frame values both encode phases read
/// (`tile_phase::decide_and_encode_tiles`, `pack_phase::filter_and_pack_frame`),
/// built once in `encode_frame_impl` after setup. None of them changes between
/// the two phases, which is what makes one copy correct. Each phase
/// destructures it on entry, so the phase bodies read the same names as the
/// inline code they came from.
#[derive(Clone, Copy)]
pub(super) struct FrameShape<'a> {
    pub(super) chroma: Option<(&'a [u8], &'a [u8])>,
    pub(super) is_key: bool,
    pub(super) sc_arm: crate::sc_detect::ScArm,
    pub(super) temporal_layer: u8,
    pub(super) frame_hier: u8,
    pub(super) w: usize,
    pub(super) h: usize,
    pub(super) fmt: svtav1_types::chroma::ChromaFormat,
    pub(super) ss_x: usize,
    pub(super) ss_y: usize,
    pub(super) acw: usize,
    pub(super) ach: usize,
    pub(super) filter_chroma: bool,
    pub(super) sc_derivation: crate::sc_detect::ScDerivation,
    pub(super) frame_tx_mode_select: bool,
    pub(super) base_qindex: u8,
    pub(super) coded_lossless: bool,
    pub(super) delta_q_plan: Option<&'a crate::sb_qindex::SbQindexPlan>,
    pub(super) md_sb_qindex: Option<&'a crate::sb_qindex::SbQindexPlan>,
    pub(super) sb_size: usize,
    pub(super) sb_cols: usize,
    pub(super) sb_rows: usize,
    pub(super) ref_padded_luma: Option<&'a crate::picture::PaddedRef>,
    pub(super) tile_grid: crate::entropy::obu::TileGrid,
    pub(super) chroma_deltas: crate::chroma_q::ChromaQDeltas,
    pub(super) qindex_u: u8,
    pub(super) qindex_v: u8,
    pub(super) qm_levels: [u8; 3],
    pub(super) seq_tools: crate::entropy::obu::SeqTools,
}

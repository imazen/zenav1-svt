use super::*;

#[allow(clippy::too_many_arguments)]
/// Mode tracking for the encoder's entropy coding context.
///
/// Tracks intra mode and skip status at 4x4 block granularity, matching
/// the decoder's above/left BlockContext arrays. This is required for
/// correct CDF context derivation in keyframe y_mode and skip coding.
///
/// Also tracks partition context at 8x8 granularity, matching the rav1d
/// decoder's `BlockContext.partition` arrays. This is essential for multi-SB
/// frames where the partition context of one SB depends on its neighbors.
#[derive(Clone, Default)]
pub(crate) struct EntropyCtx {
    /// Above row modes (at 4x4 granularity), indexed by column in 4x4 units.
    /// Updated after each block is encoded.
    pub(super) above_mode: Vec<u8>,
    /// Left column modes (at 4x4 granularity), indexed by row in 4x4 units.
    pub(super) left_mode: Vec<u8>,
    /// Above/left UV modes (4x4 granularity) — C's chroma_above/left_mbmi
    /// uv_mode inputs to `get_filt_type(xd, plane > 0)` (the intra edge
    /// filter's smooth-neighbour strength selector). Only chroma owners
    /// update these maps; their mode covers the full 8x8 luma group even
    /// when the coded owner itself is a 4x4 leaf.
    pub(super) above_uv_mode: Vec<u8>,
    pub(super) left_uv_mode: Vec<u8>,
    /// Above row skip flags.
    pub(super) above_skip: Vec<bool>,
    /// Left column skip flags.
    pub(super) left_skip: Vec<bool>,
    /// Above partition context at 8x8 granularity (full frame width).
    /// Each byte stores partition depth bits, matching rav1d's `a.partition`.
    pub(super) above_partition: Vec<u8>,
    /// Left partition context at 8x8 granularity (one SB column height).
    /// Reset at the start of each SB row, matching rav1d's `t.l.partition`.
    pub(super) left_partition: Vec<u8>,
    /// Above coefficient neighbor bytes at 4x4 granularity:
    /// `(dc_sign << 6) | min(cul_level, 63)`, 0xFF = unavailable (frame edge).
    pub(super) above_coeff: Vec<u8>,
    /// Left coefficient neighbor bytes at 4x4 granularity.
    pub(super) left_coeff: Vec<u8>,
    /// Above coefficient neighbor bytes for the chroma planes (U = 0,
    /// V = 1), in CHROMA-plane 4x4 units (each unit covers 8x8 luma
    /// pixels at 4:2:0; 4x4 at 4:4:4). Same encoding and INVALID
    /// convention as the luma arrays; the decoder keeps per-plane
    /// entropy context arrays exactly like this (libaom
    /// pd->above/left_entropy_context, zeroed per tile; 0xFF-skip ==
    /// zero contribution, matching svt_aom_get_txb_ctx).
    pub(super) above_coeff_uv: [Vec<u8>; 2],
    /// Left coefficient neighbor bytes for the chroma planes.
    pub(super) left_coeff_uv: [Vec<u8>; 2],
    /// C `subsampling_x`/`subsampling_y` of the frame's chroma format
    /// (`ChromaFormat::subsampling_x/y`) — 1/1 at 4:2:0, 0/0 at 4:4:4.
    /// Sizing base for the chroma context arrays and the chroma
    /// reference/origin rules in `record_block` / `filt_type_uv` /
    /// the residual walk. MONO callers pass a format with ss=(1,1);
    /// the chroma arrays are then sized but never read (chroma is
    /// `None` on that arm).
    pub(super) ss_x: usize,
    pub(super) ss_y: usize,
    /// Above TXFM context at 4x4 granularity: the WIDTH in pixels of the
    /// last coded TX in each mi column (C TXFM_CONTEXT / txfm_context_array
    /// top array, maintained by set_txfm_ctxs, entropy_coding.c:4614).
    /// Init value is never read: get_tx_size_context gates on
    /// availability, and every available cell was written by a previous
    /// block (blocks are coded in z-order).
    pub(super) above_txfm: Vec<u8>,
    /// IBC chunk 9: per-4x4 above-neighbour INTER block dims (0 = intra)
    /// — the get_tx_size_context is_inter override state (Root 6).
    pub(super) above_inter_bw: Vec<u8>,
    /// Left TXFM context at 4x4 granularity: the HEIGHT in pixels of the
    /// last coded TX in each mi row.
    pub(super) left_txfm: Vec<u8>,
    /// Per-4x4 left-neighbour INTER block dims (0 = intra).
    pub(super) left_inter_bh: Vec<u8>,
    /// Above row luma palette_size (4x4 granularity), 0 = no palette — C's
    /// `above_mbmi->palette_mode_info.palette_size` read back by
    /// `svt_aom_get_palette_mode_ctx` / `svt_get_palette_cache_y`. Full
    /// frame width, like `above_mode` (NOT reset per SB row — the SB-row
    /// drop rule for the color cache lives in [`palette_cache`], not here).
    pub(super) above_palette: Vec<u8>,
    /// Left column luma palette_size (4x4 granularity), 0 = no palette.
    pub(super) left_palette: Vec<u8>,
    /// Above row palette colors (4x4 granularity), aligned with
    /// `above_palette`: the first `above_palette[i]` entries of
    /// `above_palette_colors[i]` are that neighbor's ascending palette
    /// (C `above_mbmi->palette_mode_info.palette_colors`); the rest are
    /// stale/zero and MUST NOT be read.
    pub(super) above_palette_colors: Vec<[u16; svtav1_types::prediction::PALETTE_MAX_SIZE]>,
    /// Left column palette colors (4x4 granularity), aligned with
    /// `left_palette`.
    pub(super) left_palette_colors: Vec<[u16; svtav1_types::prediction::PALETTE_MAX_SIZE]>,
    /// The sequence header's `enable_filter_intra` bit (C
    /// `scs->seq_header.filter_intra_level`, read by the block walk at
    /// entropy_coding.c:5099-5100): when set, every eligible intra block
    /// (DC_PRED, no palette, both dims <= 32) codes a `use_filter_intra`
    /// symbol. Sequence-level walk config, not per-block state — carried
    /// here because the walk already threads this context everywhere.
    pub(super) seq_filter_intra: bool,
    /// FH `tx_mode == TX_MODE_SELECT` (C `frm_hdr->tx_mode`, written by
    /// `crate::txs_arm::tx_mode_select`). `av1_code_tx_size`
    /// (entropy_coding.c:4650) codes the per-block `tx_depth` symbol ONLY at
    /// TX_MODE_SELECT; at TX_MODE_LARGEST the decoder INFERS the largest tx
    /// size and the symbol must not appear.
    ///
    /// This is frame-level walk config for the same reason `seq_filter_intra`
    /// is. It was `is_key` until 2026-09-01 — a stale ALLINTRA premise (that
    /// arm signals TX_MODE_SELECT unconditionally), so on a VIDEO-mode key
    /// frame at preset >= 10, where `txs_level == 0` makes the video arm
    /// signal TX_MODE_LARGEST, the header said LARGEST and the walk still
    /// wrote a `tx_size_cdf` symbol per block. That is an undecodable stream,
    /// not merely a parity gap.
    pub(super) tx_mode_select: bool,
    /// FH `allow_screen_content_tools` — gates the per-block no-palette
    /// flag coding (C write_palette_mode_info gate, entropy_coding.c:5026).
    pub(super) allow_sct: bool,
    /// FH `allow_intrabc` — gates the per-block `use_intrabc` flag coding
    /// (C write_modes_b -> write_intrabc_info, entropy_coding.c:5021-5023;
    /// the flag is coded for EVERY block on an IBC frame). Default false;
    /// stamped post-construction (like `tile_top_px`) by the real pack walk
    /// AND the funnel chain sim — both must code it or the intrabc CDF (and
    /// every later symbol's arithmetic state) desyncs from C.
    pub(super) allow_intrabc: bool,
    /// Encoder bit depth (8 or 10) — C
    /// `ppcs->scs->static_config.encoder_bit_depth`.
    ///
    /// The pack walk needs it for the palette COLOUR literals, whose width IS
    /// the encoder bit depth (`write_palette_colors_y`,
    /// entropy_coding.c:4369 -> :4256-4288). It was hardcoded to 8, which
    /// desyncs the arithmetic decoder on the first 10-bit palette block.
    /// Passed to [`EntropyCtx::new`] rather than stamped afterwards like
    /// `allow_intrabc`: a forgotten stamp here is a silent bitstream
    /// corruption, and the compiler can enforce a parameter.
    /// ALIGNED frame extent in PIXELS (`width_4x4 * 4`). Needed by any packer
    /// that must clip a block to the part inside the frame -- see the palette
    /// map tokens, which C writes over `rows_within_bounds x cols_within_bounds`
    /// (`svt_aom_get_block_dimensions`, palette.c:217-245), not the full block.
    pub(super) aligned_w_px: usize,
    pub(super) aligned_h_px: usize,
    pub(super) bit_depth: u8,
    /// [SVT_HDR_MODE] per-SB delta-q emission state (C write_modes_b,
    /// entropy_coding.c:4997): `Some((delta_q_res, prev_qindex, sb_size))`
    /// when the FH signaled delta_q_present. The walk arms
    /// `delta_q_pending` with the SB's target qindex at each SB start; the
    /// FIRST block whose origin is the SB corner (and bsize != SB size ||
    /// !skip) emits `(cur - prev) / res` via av1_write_delta_q_index and
    /// updates prev. `sb_size` is the real superblock size — C tests the
    /// block origin against `sb_mi_size`, so at SB128 the symbol must fire
    /// once per 128x128 SB, not at every 64-boundary.
    pub delta_q_state: Option<(u8, i32, usize)>,
    /// The current SB's target qindex, set by the walk at SB start.
    pub delta_q_sb_qindex: i32,
    /// aom `--delta-lf-mode` emission state (C SVT's dead twin:
    /// `pcs->prev_delta_lf_from_base`, entropy_coding.c:3584): `Some`
    /// carries the per-TILE prev delta_lf (reset to 0 at tile start,
    /// like `prev_qindex[tile_idx]`) when the FH signaled
    /// `delta_lf_present`. The value the walk codes is
    /// [`Self::delta_lf_sb`] — the SB's `delta_lf_from_base` — reduced
    /// by `delta_lf_res` (2) exactly like the delta-qindex symbol.
    pub delta_lf_state: Option<i32>,
    /// The current SB's `delta_lf_from_base`, set by the walk at SB
    /// start off the same `sb_qindex` plan entry the delta-q symbol
    /// codes (aom `setup_delta_q`, encodeframe.c:380:
    /// `((delta_qindex/4 + res/2) & ~(res-1))` clamped, res = 2).
    pub delta_lf_sb: i32,
    /// Pending `cdef_idx` emission for the CURRENT superblock — C
    /// `write_cdef` (entropy_coding.c:3986-4017). Set at SB start by the
    /// walk when `cdef_bits > 0`, `None` otherwise.
    pub(super) cdef_sb: Option<CdefSbState>,
    /// Task #86: the Y-origin (LUMA pixel domain) of the current tile's
    /// own top row — see `PartitionSearchConfig::tile_top_px`'s doc for
    /// why this must gate "above" availability instead of frame-absolute
    /// y=0. 0 = single tile row (default, set by `EntropyCtx::new`); the
    /// per-tile entropy walk sets it explicitly per tile_idx.
    pub(crate) tile_top_px: usize,
    /// Task #96: the X-origin (LUMA pixel domain) of the current tile's
    /// own left column — the column analogue of [`Self::tile_top_px`].
    /// AV1 intra prediction and every above/left CONTEXT lookup stop at a
    /// tile boundary in BOTH axes; a block at a tile's own left column has
    /// no "left" neighbour even when it is not the frame's left column.
    /// 0 = single tile column (default), which is what every pre-#96 cell
    /// encodes, so gating on this is byte-neutral there.
    pub(crate) tile_left_px: usize,
    /// The same tile rect in LUMA mi units, INCLUDING the ends, for the MD
    /// prediction path (`intra_edge::DrGeom`'s four availability
    /// predicates need `mi_col_end` / `mi_row_end`, which the two px
    /// origins above cannot express). Defaults to the whole frame, so a
    /// single-tile encode is byte-identical. The origins and this field
    /// are assigned together at each of the (few) tile-walk sites and a
    /// debug_assert keeps them consistent.
    pub(crate) tile_mi: crate::intra_edge::TileMi,
    /// C `xd->above_mbmi` / `xd->left_mbmi` as
    /// [`crate::port_entropy_inter::block::write_inter_mode_info`] reads
    /// them, at 4x4 granularity — `docs/INTER-ENCODE-PLAN.md` §1s item 2's
    /// mi grid, restricted to the fields the inter contexts touch. Same
    /// shapes and same stamping cadence as `above_mode`/`left_mode`: the
    /// above row is frame-wide, the left column is one SB column high, and
    /// every coded block writes its own span.
    ///
    /// A `Default` entry is C's zeroed `MbModeInfo`, i.e. `DC_PRED` with
    /// `ref_frame = {0, 0}`. That is never READ: every lookup is gated on
    /// `tile_top_px` / `tile_left_px` first, so an unwritten cell is
    /// unreachable, exactly like `above_txfm`'s.
    pub(super) above_nmi: Vec<crate::port_entropy_inter::NeighborMi>,
    pub(super) left_nmi: Vec<crate::port_entropy_inter::NeighborMi>,
    /// The FULL mode-info grid `inter_mvp::setup_ref_mv_list` scans — C's
    /// `pcs->mi_grid_base` as `svt_aom_update_mi_map` leaves it
    /// (`docs/INTER-ENCODE-PLAN.md` §1s item 2). The above/left rows above
    /// are the two cells the entropy CONTEXTS read; the MVP walk reads rows
    /// -1..-3, columns -1..-3 and the top-right cell, so it needs the grid.
    ///
    /// Empty on a key frame, where every MVP scan is the IntraBC one
    /// (`crate::intrabc_mvp`, which keeps its own grid in the funnel).
    pub(super) mvp_grid: Vec<crate::intrabc_mvp::MvpMiEntry>,
    /// The frame-constant half of the MVP environment. `Some` exactly when
    /// [`Self::inter_syntax`] is.
    pub(crate) mvp_env: Option<crate::partition::InterMdEnv>,
    /// The frame-level inter syntax the pack's inter arm needs. `Some`
    /// exactly on a non-key frame; the arm refuses without it rather than
    /// inventing a header it cannot have read.
    pub(crate) inter_syntax: Option<InterSyntaxState>,
    /// C's `update_b` coded-area accumulators (`coding_loop.c:1605-1643`),
    /// `Some` exactly when `!scs->allintra` — i.e. on the VIDEO arm, which is
    /// the only arm whose pictures are ever read as references.
    ///
    /// They live here rather than in a fold over the decided trees because
    /// the SKIP half is `blk_ptr->block_has_coeff`, and the walk is the one
    /// place that already computes it (the `skip` symbol it writes IS
    /// `!block_has_coeff`). A second derivation would be the campaign's
    /// sixth duplicate transcription — see `docs/WORKING-ON-THIS.md` 4.
    pub(crate) coded_area: Option<CodedAreaAcc>,
}

/// C's per-picture coded-area accumulators, as `update_b` builds them
/// (`coding_loop.c:1605-1643`) and `enc_dec_process.c:3167-3169` sums them.
///
/// AREAS in luma pixels here; `rest_process.c:347-349` turns them into
/// percentages, which is what the DPB entry carries.
#[derive(Clone, Debug)]
pub(crate) struct CodedAreaAcc {
    /// C `frm_hdr->allow_high_precision_mv` — the gate on the hp accumulator.
    pub(crate) allow_high_precision_mv: bool,
    /// Superblock size in luma pixels, for the `sb_index` of a block origin.
    pub(crate) sb_size: usize,
    /// Superblocks per row.
    pub(crate) sb_cols: usize,
    /// C `ctx->tot_intra_coded_area`.
    pub(crate) intra_area: u64,
    /// C `ctx->tot_skip_coded_area`.
    pub(crate) skip_area: u64,
    /// C `ctx->tot_hp_coded_area`.
    pub(crate) hp_area: u64,
    /// C `ctx->tot_cnt_zero_mv` (coding_loop.c:1626-1627) — the pixel area
    /// of blocks whose MV is below half-pel, normalized into
    /// `pcs->avg_cnt_zeromv` by `rest_process.c:350` and folded into
    /// `rc->avg_frame_low_motion` by `svt_av1_rc_postencode_update`
    /// (rc_vbr_cbr.c:1617-1619). Only CBR reads it.
    pub(crate) zeromv_area: u64,
    /// C `pcs->sb_intra[sb]`, init 0 (`enc_dec_process.c:3099`).
    pub(crate) sb_intra: Vec<u8>,
    /// C `pcs->sb_skip[sb]`, init **1** (`enc_dec_process.c:3100`).
    pub(crate) sb_skip: Vec<u8>,
    /// C `pcs->sb_64x64_mvp[sb]`, init 0 (`enc_dec_process.c:3123`). Set when
    /// the committed superblock is one 64x64 inter block on a non-new-MV
    /// mode (`coding_loop.c:1629`); copied to the reference object so a later
    /// frame's `svt_aom_sig_deriv_enc_dec_light_pd1_default` can read it.
    pub(crate) sb_64x64_mvp: Vec<u8>,
    /// C `EbReferenceObject::mvs`, the MFMV writeback `update_b` performs
    /// alongside the three areas (`coding_loop.c:1748-1758`). EMPTY unless
    /// [`Self::mfmv_active`] — C's own gate is
    /// `scs->mfmv_enabled && slice_type != I_SLICE && ppcs->is_ref`, which is
    /// false on every key frame and every allintra cell.
    pub(crate) mvs: Vec<crate::inter_mvp::MvRef>,
    /// C `pcs->ref_frame_side`, which `av1_copy_frame_mvs` reads as a VETO
    /// per reference slot. Produced by
    /// [`crate::inter_mvp::setup_motion_field`] at picture level, so there is
    /// one derivation and the walk consumes it.
    pub(crate) ref_frame_side: [i8; 8],
    /// C `cm->mi_cols` — the stride arithmetic of [`Self::mvs`].
    pub(crate) mi_cols: i32,
    /// C `cm->mi_rows`, for the clamp on `y_mis`.
    pub(crate) mi_rows: i32,
    /// C's `if (pcs->scs->mfmv_enabled && pcs->slice_type != I_SLICE &&
    /// pcs->ppcs->is_ref)` at `coding_loop.c:1748`.
    pub(crate) mfmv_active: bool,
}

impl CodedAreaAcc {
    /// C `av1_copy_frame_mvs`'s per-cell reset (`coding_loop.c:1049-1050`),
    /// and therefore the value every untouched cell of the field holds.
    const MV_RESET: crate::inter_mvp::MvRef = crate::inter_mvp::MvRef {
        mv: svtav1_types::motion::Mv { x: 0, y: 0 },
        ref_frame: crate::port_coding_loop::NONE_FRAME,
    };

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        allow_high_precision_mv: bool,
        sb_size: usize,
        sb_cols: usize,
        sb_rows: usize,
        mi_rows: i32,
        mi_cols: i32,
        mfmv_active: bool,
        ref_frame_side: [i8; 8],
    ) -> Self {
        // C allocates `mvs` at HALF mi resolution in both dimensions, and
        // `av1_copy_frame_mvs` / `motion_field_projection` both index it with
        // stride `ROUND_POWER_OF_TWO(mi_cols, 1)`.
        //
        // ALLOCATED UNCONDITIONALLY, not only when [`Self::mfmv_active`].
        // C's `EbReferenceObject::mvs` exists on every reference object and is
        // simply not WRITTEN when the gate is false, leaving zeros — which
        // `motion_field_projection` skips (`ref_frame > INTRA_FRAME` is
        // false). A port that allocated nothing there instead handed that
        // function a SHORT SLICE, and it indexes before it can know: MEASURED
        // by mutation, forcing `mfmv_active` false panicked
        // `inter_mvp.rs:2266` on both `refuses_inter3` cells. The reset value
        // is `NONE_FRAME` rather than C's zero; both are `<= INTRA_FRAME`, so
        // the projection behaves identically.
        let mvs_len = (((mi_rows + 1) >> 1) * ((mi_cols + 1) >> 1)) as usize;
        Self {
            allow_high_precision_mv,
            sb_size,
            sb_cols,
            intra_area: 0,
            skip_area: 0,
            hp_area: 0,
            zeromv_area: 0,
            sb_intra: alloc::vec![0u8; sb_cols * sb_rows],
            sb_skip: alloc::vec![1u8; sb_cols * sb_rows],
            sb_64x64_mvp: alloc::vec![0u8; sb_cols * sb_rows],
            mvs: alloc::vec![Self::MV_RESET; mvs_len],
            ref_frame_side,
            mi_cols,
            mi_rows,
            mfmv_active,
        }
    }

    /// Fold another tile's accumulators in. C accumulates per EncDec context
    /// and sums under `pcs->intra_mutex` (`enc_dec_process.c:3166-3170`); the
    /// per-superblock flags are written to disjoint entries, so a max/min is
    /// the same thing as C's single shared array.
    pub(crate) fn merge(&mut self, other: &Self) {
        self.intra_area += other.intra_area;
        self.skip_area += other.skip_area;
        self.hp_area += other.hp_area;
        self.zeromv_area += other.zeromv_area;
        for (d, s) in self.sb_intra.iter_mut().zip(&other.sb_intra) {
            *d |= *s;
        }
        for (d, s) in self.sb_skip.iter_mut().zip(&other.sb_skip) {
            *d &= *s;
        }
        // `sb_64x64_mvp` is a per-SB flag written to a disjoint cell by the
        // tile that owns it — `|` is C's shared-array write.
        for (d, s) in self.sb_64x64_mvp.iter_mut().zip(&other.sb_64x64_mvp) {
            *d |= *s;
        }
        // Each tile writes DISJOINT cells of the motion field — a coded block
        // belongs to exactly one tile — and both sides start at C's own reset
        // value (`NONE_FRAME`, zero MV), which is also what a block with no
        // usable reference leaves. So "take the other side when it is not the
        // reset value" is C's single shared array: a cell this tile owns and
        // wrote as the reset value is indistinguishable from one it never
        // owned, and both answers are the same value. Same argument as the
        // two flag arrays above.
        for (d, s) in self.mvs.iter_mut().zip(&other.mvs) {
            if *s != Self::MV_RESET {
                *d = *s;
            }
        }
    }

    /// C `update_b`, for ONE coded block (`coding_loop.c:1605-1643`).
    ///
    /// `skip` is C's `blk_ptr->block_has_coeff == 0`, which is exactly the
    /// `skip` symbol the walk writes.
    pub(crate) fn add_block(
        &mut self,
        decision: &crate::partition::BlockDecision,
        block_x: usize,
        block_y: usize,
        skip: bool,
    ) {
        let area = u64::from(decision.width) * u64::from(decision.height);
        let sb_idx = (block_y / self.sb_size) * self.sb_cols + (block_x / self.sb_size);
        if decision.is_inter {
            // C reads `blk_ptr->block_mi.mv[0]` (and mv[1] on a compound
            // block) and asks whether EITHER component is odd, i.e. whether
            // the eighth-pel MV is not a quarter-pel one. Gated on
            // `allow_high_precision_mv` because C is.
            if self.allow_high_precision_mv
                && let Some(i) = decision.inter.as_ref()
            {
                let odd = |m: svtav1_types::motion::Mv| m.x % 2 != 0 || m.y % 2 != 0;
                let mut hp = odd(i.mv[0]);
                if !hp && i.ref_frame[1] > 0 {
                    hp = odd(i.mv[1]);
                }
                if hp {
                    self.hp_area += area;
                }
            }
            // C `coding_loop.c:1617-1628` — `tot_cnt_zero_mv`: the block's
            // pixel area counts when its list-0 MV is within half-pel, OR
            // (a compound block) its list-1 MV is. C's test is on the
            // eighth-pel `block_mi.mv`, which is what `InterDecision::mv`
            // carries.
            if let Some(i) = decision.inter.as_ref() {
                let sub_half_pel = |m: svtav1_types::motion::Mv| {
                    i32::from(m.x).abs() < 8 && i32::from(m.y).abs() < 8
                };
                if sub_half_pel(i.mv[0]) || (i.ref_frame[1] > 0 && sub_half_pel(i.mv[1])) {
                    self.zeromv_area += area;
                }
            }
            // C `coding_loop.c:1629` — the same non-intra arm. A single
            // 64x64 inter block (`sq_size == sb_size`) on a non-new-MV mode
            // flags this SB for a later frame's light-PD1 signal derivation.
            if decision.width as usize == self.sb_size
                && decision.height as usize == self.sb_size
                && let Some(i) = decision.inter.as_ref()
                && !matches!(
                    i.mode,
                    svtav1_types::prediction::PredictionMode::NewMv
                        | svtav1_types::prediction::PredictionMode::NewNewMv
                )
                && let Some(f) = self.sb_64x64_mvp.get_mut(sb_idx)
            {
                *f = 1;
            }
        } else {
            // C `is_intra_mode(blk_ptr->block_mi.mode)`. An IntraBC block
            // codes DC_PRED and is therefore INTRA to this accumulator, which
            // is what `!decision.is_inter` says (see `BlockDecision`'s
            // `use_intrabc`: the two are independent).
            self.intra_area += area;
            if let Some(f) = self.sb_intra.get_mut(sb_idx) {
                *f = 1;
            }
        }
        if skip {
            self.skip_area += area;
        } else if let Some(f) = self.sb_skip.get_mut(sb_idx) {
            *f = 0;
        }
        // C `update_b`'s MFMV writeback (`coding_loop.c:1747-1758`), in the
        // same function and under C's own gate. It runs for EVERY coded block,
        // intra included — an intra block's `ref_frame` is not `> INTRA_FRAME`
        // so `copy_frame_mvs` resets its cells, which is exactly how a later
        // frame stops projecting stale motion through a block that has none.
        if self.mfmv_active {
            let mi_row = (block_y / 4) as i32;
            let mi_col = (block_x / 4) as i32;
            // C `AOMMIN(blk_geom->bwidth >> MI_SIZE_LOG2, mi_cols - mi_col)`.
            let x_mis = ((i32::from(decision.width)) >> 2).min(self.mi_cols - mi_col);
            let y_mis = ((i32::from(decision.height)) >> 2).min(self.mi_rows - mi_row);
            let (ref_frame, mv) = decision.inter.as_ref().map_or(
                (
                    [crate::port_coding_loop::NONE_FRAME; 2],
                    [svtav1_types::motion::Mv { x: 0, y: 0 }; 2],
                ),
                |i| (i.ref_frame, i.mv),
            );
            crate::port_coding_loop::copy_frame_mvs(
                &mut self.mvs,
                self.mi_cols,
                ref_frame,
                mv,
                &self.ref_frame_side,
                mi_row,
                mi_col,
                x_mis,
                y_mis,
            );
        }
    }

    /// C `rest_process.c:347-349`: `100 * area / (aligned_w * aligned_h)`,
    /// with `intra_coded_area` forced to 0 on an I_SLICE. Returns
    /// `(intra, skip, hp)` as the `uint8_t` percentages the reference object
    /// stores (`copy_statistics_to_ref_obj_ect`, :195-197).
    pub(crate) fn percentages(
        &self,
        aligned_w: usize,
        aligned_h: usize,
        is_islice: bool,
    ) -> (u8, u8, u8) {
        let n = (aligned_w * aligned_h) as u64;
        if n == 0 {
            return (0, 0, 0);
        }
        let pct = |a: u64| (100 * a / n) as u8;
        (
            if is_islice { 0 } else { pct(self.intra_area) },
            pct(self.skip_area),
            pct(self.hp_area),
        )
    }
}

/// The owned twin of
/// [`crate::port_entropy_inter::block::InterFrameSyntax`], which borrows its
/// two tables. Held per frame by [`EntropyCtx`] and lent out per block.
#[derive(Clone, Debug)]
pub(crate) struct InterSyntaxState {
    /// C `frm_hdr->skip_mode_params.skip_mode_flag`
    /// (`pd_process.c:4958` = `skip_mode_allowed`). Gates BOTH the header bit
    /// and the per-block `skip_mode` symbol (`entropy_coding.c:5119`).
    pub skip_mode_flag: bool,
    /// C `frm_hdr->skip_mode_params.ref_frame_idx_{0,1}` — the pair a
    /// NEAREST_NEARESTMV candidate must carry for `skip_mode_allowed` to
    /// fire (mode_decision.c:1590-1594). `INVALID_IDX` (-1) when the frame
    /// does not allow skip mode.
    pub skip_mode_ref_frame_idx_0: i8,
    pub skip_mode_ref_frame_idx_1: i8,
    pub reference_mode: crate::port_entropy_inter::refframe::ReferenceMode,
    pub interpolation_filter: u8,
    pub enable_dual_filter: bool,
    pub enable_interintra_compound: bool,
    pub enable_masked_compound: bool,
    pub enable_jnt_comp: bool,
    pub enable_order_hint: bool,
    pub order_hint_bits: u32,
    pub is_motion_mode_switchable: bool,
    pub allow_warped_motion: bool,
    pub allow_high_precision_mv: bool,
    pub force_integer_mv: bool,
    pub gm_wmtype: [crate::port_entropy_inter::modes::TransformationType; 8],
    pub cur_order_hint: i32,
    pub ref_order_hint: [i32; 7],
    /// C `frm_hdr->use_ref_frame_mvs`. NOT part of
    /// [`crate::port_entropy_inter::block::InterFrameSyntax`] — the entropy
    /// walk never reads it — but the MVP walk does, and it is the same
    /// frame-header bit derived from the same signals, so it is carried
    /// with them rather than re-derived somewhere the header cannot see.
    pub use_ref_frame_mvs: bool,
}

impl InterSyntaxState {
    pub(crate) fn syntax(&self) -> crate::port_entropy_inter::block::InterFrameSyntax<'_> {
        crate::port_entropy_inter::block::InterFrameSyntax {
            reference_mode: self.reference_mode,
            interpolation_filter: self.interpolation_filter,
            enable_dual_filter: self.enable_dual_filter,
            enable_interintra_compound: self.enable_interintra_compound,
            enable_masked_compound: self.enable_masked_compound,
            enable_jnt_comp: self.enable_jnt_comp,
            enable_order_hint: self.enable_order_hint,
            order_hint_bits: self.order_hint_bits,
            is_motion_mode_switchable: self.is_motion_mode_switchable,
            allow_warped_motion: self.allow_warped_motion,
            allow_high_precision_mv: self.allow_high_precision_mv,
            force_integer_mv: self.force_integer_mv,
            gm_wmtype: &self.gm_wmtype,
            cur_order_hint: self.cur_order_hint,
            ref_order_hint: &self.ref_order_hint,
        }
    }
}

/// C `write_cdef`'s per-superblock state (entropy_coding.c:3986-4017).
///
/// The CDEF filter block is 64x64 **always**, so an SB128 superblock covers
/// FOUR of them and C emits up to four `cdef_bits` literals per SB — one at
/// the first non-skip coding block of each quadrant — latched by
/// `cdef_transmitted[4]`:
///
/// ```text
/// const int32_t mask  = 1 << (6 - MI_SIZE_LOG2);            // 16 mi = 64 px
/// const int32_t index = sb_size == BLOCK_128X128
///     ? !!(mi_col & mask) + 2 * !!(mi_row & mask) : 0;
/// if (!ctx->cdef_transmitted[index] && !skip) {
///     aom_write_literal(w, mbmi->cdef_strength, cdef_bits);
///     ctx->cdef_transmitted[index] = true;
/// }
/// ```
///
/// The strength itself is read off the **b64 grid** — C takes the mbmi at
/// `(mi_row & ~15, mi_col & ~15)`, i.e. the 64-aligned mi — which is what
/// [`Self::strengths`] caches per quadrant.
///
/// At SB64 there is exactly one quadrant, `index` is always 0, and this is
/// bit-for-bit the previous single-slot behaviour.
///
/// NOTE the three-phase CDEF contract that docs/sb128-port-map.md flags as
/// the highest-risk SB128 chunk (search skips stale quadrants / strengths
/// fan out to covered quadrants / dirinit forced fresh) collapses to a
/// no-op here, because on a KEY frame the 128 root is ALWAYS split (see
/// `merge_sb_units`) so NO coding block is ever a 128-variant. Every 64x64
/// filter block owns its own blocks and its own searched strength, exactly
/// as at SB64. Only this WRITE side differs.
#[derive(Clone, Copy, Debug)]
pub(super) struct CdefSbState {
    /// C `cdef_bits` (> 0, else the walk stores `None`).
    pub(super) bits: u8,
    /// Per-quadrant strength index, b64-grid order (0=TL, 1=TR, 2=BL, 3=BR).
    pub(super) strengths: [u8; 4],
    /// C `ctx->cdef_transmitted[4]`, reset at each SB top-left.
    pub(super) transmitted: [bool; 4],
    /// SB128: quadrant index varies. SB64: always slot 0.
    pub(super) sb128: bool,
}

/// Live state for the 4:2:0 chroma pass, threaded through the entropy walk
/// so every leaf's chroma blocks are predicted from — and reconstructed
/// into — the chroma planes in exact coding order (identical to the
/// decoder's parse order; the walk IS the bitstream order).
pub(super) struct ChromaPass<'a> {
    pub(super) u_src: &'a [u8],
    pub(super) v_src: &'a [u8],
    pub(super) u_recon: &'a mut [u8],
    pub(super) v_recon: &'a mut [u8],
    /// Chroma plane stride (= frame_width / 2).
    pub(super) stride: usize,
    /// Per-plane chroma quantization qindexes: clamp(base + FH
    /// delta_q_ac[plane]). Both == base_qindex in mainline mode (all FH
    /// chroma deltas 0); the fork's chroma-q path sets them independently
    /// and the FH signals the deltas (chroma_q.rs).
    pub(super) qindex_u: u8,
    pub(super) qindex_v: u8,
    /// [SVT_HDR_MODE] per-plane chroma QM levels (15 = off).
    pub(super) qm_u: u8,
    pub(super) qm_v: u8,
    /// Frame-level C-exact coding quantizer (still path) — C's MDS3 RDOQ
    /// covers chroma too (skip_uv cleared when enc-dec is bypassed).
    pub(super) c_quant: Option<&'a crate::quant::CodingQuantCfg>,
    /// The reference's padded chroma planes — `Some` exactly when the frame
    /// can code inter chroma (a non-key frame whose DPB slot carries chroma).
    /// An inter leaf with no `chroma_dec` residual-codes against the MC
    /// prediction built from these; `None` only where inter cannot occur.
    pub(super) ref_uv: Option<(
        &'a crate::picture::PaddedPlane,
        &'a crate::picture::PaddedPlane,
    )>,
    /// `scs->super_block_size` for `clamp_mv_to_umv_border_sb` — the MV's
    /// out-of-frame bound is against the SB, not the block.
    pub(super) sb_size: usize,
    /// LUMA frame dims (coded extent) — `mb_edges`/`RefGeometry` are
    /// luma-domain; the chroma plane dims follow from `ss_x`/`ss_y`.
    pub(super) frame_w: usize,
    pub(super) frame_h: usize,
}

/// One coded block's chroma coefficient work: the chroma plane block at
/// (`cx`,`cy`) chroma px of `cw`x`ch`, emitted as a clipped grid of
/// `txw`x`txh` TXBs in decoder `residual()` order (plane-major, raster).
/// `u`/`v` carry one `(qcoeffs, eob)` per TXB in that order.
pub(super) struct ChromaBlock {
    pub(super) cx: usize,
    pub(super) cy: usize,
    pub(super) cw: usize,
    pub(super) ch: usize,
    /// Per-TXB dims in chroma px (== `cw`x`ch` when one TXB covers the
    /// plane block — the whole 4:2:0 surface and most of 4:4:4).
    pub(super) txw: usize,
    pub(super) txh: usize,
    /// In-frame TXB grid bounds in chroma 4x4 units (`max_block_units_ss`
    /// clip) — the `u`/`v` vecs hold `ceil(bw_units/txw4) *
    /// ceil(bh_units/txh4)` entries in raster order.
    pub(super) bw_units: usize,
    pub(super) bh_units: usize,
    /// `num_pels(plane_bsize) > num_pels(txb)` — the +10 vs +7 chroma
    /// `txb_skip_ctx` offset (libaom `get_txb_ctx` plane > 0 arm).
    pub(super) larger: bool,
    pub(super) u: alloc::vec::Vec<(alloc::vec::Vec<i32>, u16)>,
    pub(super) v: alloc::vec::Vec<(alloc::vec::Vec<i32>, u16)>,
}

/// Partition context update lookup table, matching rav1d's `dav1d_al_part_ctx`.
///
/// Indexed as `AL_PART_CTX[direction][block_level][partition_type]`.
/// direction: 0 = above, 1 = left.
/// block_level: 0 = Bl128x128, 1 = Bl64x64, 2 = Bl32x32, 3 = Bl16x16, 4 = Bl8x8.
/// partition_type: 0=NONE, 1=HORZ, 2=VERT, 3=SPLIT, 4-9=extended.
/// Value 0xff marks invalid combinations (SPLIT doesn't update directly).
pub(super) static AL_PART_CTX: [[[u8; 10]; 5]; 2] = [
    // Above context
    [
        [0x00, 0x00, 0x10, 0xff, 0x00, 0x10, 0x10, 0x10, 0xff, 0xff], // Bl128x128
        [0x10, 0x10, 0x18, 0xff, 0x10, 0x18, 0x18, 0x18, 0x10, 0x1c], // Bl64x64
        [0x18, 0x18, 0x1c, 0xff, 0x18, 0x1c, 0x1c, 0x1c, 0x18, 0x1e], // Bl32x32
        [0x1c, 0x1c, 0x1e, 0xff, 0x1c, 0x1e, 0x1e, 0x1e, 0x1c, 0x1f], // Bl16x16
        [0x1e, 0x1e, 0x1f, 0x1f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff], // Bl8x8
    ],
    // Left context
    [
        [0x00, 0x10, 0x00, 0xff, 0x10, 0x10, 0x00, 0x10, 0xff, 0xff], // Bl128x128
        [0x10, 0x18, 0x10, 0xff, 0x18, 0x18, 0x10, 0x18, 0x1c, 0x10], // Bl64x64
        [0x18, 0x1c, 0x18, 0xff, 0x1c, 0x1c, 0x18, 0x1c, 0x1e, 0x18], // Bl32x32
        [0x1c, 0x1e, 0x1c, 0xff, 0x1e, 0x1e, 0x1c, 0x1e, 0x1f, 0x1c], // Bl16x16
        [0x1e, 0x1f, 0x1e, 0x1f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff], // Bl8x8
    ],
];

impl EntropyCtx {
    pub(crate) fn new(
        width_4x4: usize,
        height_4x4: usize,
        seq_filter_intra: bool,
        // FH `tx_mode == TX_MODE_SELECT` — see the field's doc.
        tx_mode_select: bool,
        allow_sct: bool,
        bit_depth: u8,
        chroma_format: svtav1_types::chroma::ChromaFormat,
    ) -> Self {
        let width_8x8 = width_4x4.div_ceil(2);
        let height_8x8 = height_4x4.div_ceil(2);
        let ss_x = chroma_format.subsampling_x() as usize;
        let ss_y = chroma_format.subsampling_y() as usize;
        // Chroma-plane 4x4 units: chroma px = aligned >> ss, so units =
        // width_4x4 >> ss (frames are 8-aligned so the 420 halving divides
        // exactly; div_ceil for safety on the general axis).
        let width_c4 = width_4x4.div_ceil(1 << ss_x);
        let height_c4 = height_4x4.div_ceil(1 << ss_y);
        Self {
            aligned_w_px: width_4x4 * 4,
            aligned_h_px: height_4x4 * 4,
            above_mode: alloc::vec![0u8; width_4x4], // DC_PRED = 0
            left_mode: alloc::vec![0u8; height_4x4],
            above_uv_mode: alloc::vec![0u8; width_4x4],
            left_uv_mode: alloc::vec![0u8; height_4x4],
            above_skip: alloc::vec![false; width_4x4],
            left_skip: alloc::vec![false; height_4x4],
            above_partition: alloc::vec![0u8; width_8x8],
            left_partition: alloc::vec![0u8; height_8x8],
            // 0xFF = INVALID_NEIGHBOR_DATA at frame edges, like C's
            // neighbor-array init.
            above_coeff: alloc::vec![0xFFu8; width_4x4],
            left_coeff: alloc::vec![0xFFu8; height_4x4],
            above_coeff_uv: [alloc::vec![0xFFu8; width_c4], alloc::vec![0xFFu8; width_c4]],
            left_coeff_uv: [
                alloc::vec![0xFFu8; height_c4],
                alloc::vec![0xFFu8; height_c4],
            ],
            // C inits the TXFM neighbour arrays to NEIGHBOR_ARRAY_INVALID
            // (0xFF, neighbor_arrays.h:30 / svt_aom_neighbor_array_unit_reset).
            // The intra tx_size ctx never sees it (availability-gated), but
            // the IBC var-tx `txfm_partition_context` reads the RAW byte
            // with NO availability gate (`*above_ctx < txw`,
            // entropy_coding.c:4490) — a 0 init flips a/l to 1 at
            // tile-top/left blocks and desyncs the txfm_partition CDF row
            // vs the decoder (the chunk-8 gui corruption root).
            above_txfm: alloc::vec![0xFFu8; width_4x4],
            above_inter_bw: alloc::vec![0u8; width_4x4],
            left_txfm: alloc::vec![0xFFu8; height_4x4],
            left_inter_bh: alloc::vec![0u8; height_4x4],
            above_palette: alloc::vec![0u8; width_4x4],
            left_palette: alloc::vec![0u8; height_4x4],
            above_palette_colors: alloc::vec![
                [0u16; svtav1_types::prediction::PALETTE_MAX_SIZE];
                width_4x4
            ],
            left_palette_colors: alloc::vec![
                [0u16; svtav1_types::prediction::PALETTE_MAX_SIZE];
                height_4x4
            ],
            seq_filter_intra,
            tx_mode_select,
            allow_sct,
            bit_depth,
            ss_x,
            ss_y,
            allow_intrabc: false,
            delta_q_state: None,
            delta_q_sb_qindex: 0,
            delta_lf_state: None,
            delta_lf_sb: 0,
            cdef_sb: None,
            tile_top_px: 0,
            tile_left_px: 0,
            tile_mi: crate::intra_edge::TileMi {
                mi_row_start: 0,
                mi_row_end: height_4x4,
                mi_col_start: 0,
                mi_col_end: width_4x4,
            },
            above_nmi: alloc::vec![
                crate::port_entropy_inter::NeighborMi::default();
                width_4x4
            ],
            left_nmi: alloc::vec![
                crate::port_entropy_inter::NeighborMi::default();
                height_4x4
            ],
            inter_syntax: None,
            // Armed by the caller on the VIDEO arm only (`CodedAreaAcc`).
            coded_area: None,
            mvp_grid: Vec::new(),
            mvp_env: None,
        }
    }

    /// Allocate the mode-info grid for a frame that has references
    /// (`docs/INTER-ENCODE-PLAN.md` §1s item 2). Separate from `new` because
    /// a key frame must NOT pay for it — every still cell in the 1,100-cell
    /// envelope goes through the same constructor.
    pub(crate) fn arm_inter_mvp(&mut self, env: crate::partition::InterMdEnv) {
        self.mvp_grid = alloc::vec![
            crate::intrabc_mvp::MvpMiEntry::default();
            (env.mi_rows * env.mi_stride) as usize
        ];
        self.mvp_env = Some(env);
    }

    /// The three fields C caches on `BlkStruct` for the entropy coder —
    /// `predmv`, `inter_mode_ctx` and `drl_ctx`/`drl_ctx_near` — derived
    /// HERE from the committed mode-info map instead of carried from MD.
    /// See [`crate::partition::InterDecision`] for why.
    ///
    /// Returns `None` when the frame has no MVP environment, which is
    /// exactly when no inter block can exist.
    pub(crate) fn inter_mvp_fields(
        &self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        d: &crate::partition::InterDecision,
    ) -> Option<(
        [svtav1_types::motion::Mv; 2],
        i16,
        crate::port_entropy_inter::modes::DrlBlock,
        u32,
        u16,
    )> {
        let env = self.mvp_env.as_ref()?;
        let (mi_row, mi_col) = ((y / 4) as i32, (x / 4) as i32);
        let bsize = crate::leaf_funnel::c_bsize_index(w, h);
        let ctx = crate::intrabc_mvp::derive_block_ctx(
            mi_row,
            mi_col,
            bsize,
            env.mi_rows,
            env.mi_cols,
            env.tile,
            env.sb_mi_size,
        );
        let grid = crate::intrabc_mvp::MvpGrid {
            entries: &self.mvp_grid,
            stride: env.mi_stride,
            base: mi_row * env.mi_stride + mi_col,
        };
        // C `svt_aom_generate_av1_mvp_table`'s `gm_mv` — the block-centre
        // projection of THIS reference's global-motion model
        // (adaptive_mv_pred.c:1372-1394).
        //
        // This was a hardcoded ZERO with a comment saying the header refuses
        // any non-identity model. The header no longer refuses one, and a
        // zero here is not a conservative default: `setup_ref_mv_list` FILLS
        // the tail of the stack with `gm_mv[0]` (:1310), so a frame with a
        // real model differences every under-populated block's MV against a
        // predictor the DECODER does not share. MEASURED on `crop:` CID22 256
        // at a 33/32 zoom, preset 2: block mi(0,0) coded `pmv=(0,0)` where the
        // decoder rebuilds (30,30), and 1557 of 4096 mi units diverged.
        let ref_frame_type = crate::inter_mvp::av1_ref_frame_type(d.ref_frame);
        let mvp_env = env
            .mvp_env()
            // C `scs->super_block_size`, which the MVP environment carries as
            // `sb_mi_size` in mi units (16 for a 64x64 superblock).
            .for_block(env.sb_mi_size as usize * 4, w, h);
        let gm_mv =
            crate::inter_mvp::gm_mv_candidates_for(&mvp_env, ref_frame_type, bsize, mi_col, mi_row);
        // C's `symteric_refs` shortcut has the BWDREF / LAST_BWD passes READ
        // `mv_ref0` slots the LAST_FRAME pass wrote — inside ONE
        // `svt_aom_generate_av1_mvp_table` call sharing the scratch across
        // its ref loop. Rebuilding a single ref's stack here with a zeroed
        // scratch would read `-0` where C (and the decoder) read the LAST
        // projection, so seed it with a LAST pass first exactly as the
        // driver does. LAST_FRAME is always in the list when the gate
        // fired ({LAST, BWDREF, LAST_BWD}).
        //
        // The seeded build is also value-correct for a block whose OWN
        // `symteric_refs` evaluated false (a `determine_best_references`
        // reorder): the mirror is the sign-exact negation of the
        // projection the non-symmetric arm computes, so either path
        // yields the same stack C left on `ctx->ref_mv_stack`.
        let stack =
            if mvp_env.symmetric_refs_eligible && ref_frame_type != crate::inter_mvp::LAST_FRAME {
                let mut seeded_env = mvp_env;
                seeded_env.symmetric_refs = true;
                let gm_last = crate::inter_mvp::gm_mv_candidates_for(
                    &seeded_env,
                    crate::inter_mvp::LAST_FRAME,
                    bsize,
                    mi_col,
                    mi_row,
                );
                let last = crate::inter_mvp::setup_ref_mv_list(
                    &grid,
                    &ctx,
                    &seeded_env,
                    crate::inter_mvp::LAST_FRAME,
                    gm_last,
                );
                crate::inter_mvp::setup_ref_mv_list_seeded(
                    &grid,
                    &ctx,
                    &seeded_env,
                    ref_frame_type,
                    gm_mv,
                    last.mv_ref0,
                )
            } else {
                crate::inter_mvp::setup_ref_mv_list(&grid, &ctx, &mvp_env, ref_frame_type, gm_mv)
            };
        let pred = crate::inter_mvp::get_av1_mv_pred_drl(
            &stack,
            d.ref_frame[1] > 0,
            d.mode as u8,
            usize::from(d.drl_index),
            crate::inter_mvp::DrlMvPred::default(),
        );
        // C `mode_decision.c:3709-3728` — the two loops that fill
        // `drl_ctx` (0..2, NEWMV family) and `drl_ctx_near` (1..3, NEARMV
        // family), already ported as `port_md_winner::winner_signals`'s
        // `drl_contexts`.
        let (drl_ctx, drl_ctx_near) =
            crate::port_md_winner::drl_contexts_for(d.mode as u8, stack.count, &stack.stack);
        // `motion_mode_allowed`'s two data inputs, derived HERE from the same
        // committed grid for exactly the reason `predmv`/`drl_ctx` are: the
        // DECODER recomputes both per block (decodemv.c:1448-1455 —
        // `av1_findSamples` + `av1_count_overlappable_neighbors`), and an
        // MD-carried count that went stale — or a hardcoded 0 on a path that
        // never computed them — writes the wrong motion-mode ALPHABET and
        // desyncs the tile. MEASURED: the 4:4:4 non-funnel inter path stamped
        // `overlappable_neighbors = 0`, so a `WARPED_CAUSAL`-allowed
        // three-symbol `motion_mode` went uncoded on every neighboured block
        // and aomdec rejected the tile.
        let overlappable_neighbors =
            crate::inter_mvp::count_overlappable_neighbors(&grid, &ctx, bsize);
        // C's gate on the findSamples call (decodemv.c:1448): the count is 0
        // for a skip-mode or second-ref block regardless of neighbours, and
        // the bsize gate lives inside `count_overlappable_neighbors` already
        // — mirror all three so a compound/interintra future block cannot
        // silently inherit a nonzero count.
        let num_proj_ref = if !d.skip_mode
            && d.ref_frame[1] <= crate::inter_mvp::INTRA_FRAME
            && crate::port_entropy_inter::modes::is_motion_variation_allowed_bsize_idx(bsize)
        {
            crate::inter_mvp::find_warp_samples(&grid, &ctx, d.ref_frame[0]).0 as u16
        } else {
            0
        };
        Some((
            pred.ref_mv,
            stack.mode_context,
            crate::port_entropy_inter::modes::DrlBlock {
                drl_ctx,
                drl_ctx_near,
                drl_index: d.drl_index,
            },
            overlappable_neighbors,
            num_proj_ref,
        ))
    }

    /// C `set_mi_row_col`'s `above_mbmi` / `left_mbmi` pair for a block at
    /// `(x, y)`, with the tile-boundary availability the rest of this type
    /// already models (`tile_top_px` / `tile_left_px`).
    ///
    /// The pointer and the availability flag are SEPARATE knobs in C, and
    /// [`crate::port_entropy_inter::Neighbors`] keeps them separate for the
    /// reason its doc gives; a caller that collapsed them would change the
    /// reference-count contexts.
    pub(crate) fn inter_neighbors(
        &self,
        x: usize,
        y: usize,
    ) -> crate::port_entropy_inter::Neighbors {
        let up = y > self.tile_top_px;
        let le = x > self.tile_left_px;
        crate::port_entropy_inter::Neighbors {
            above: if up {
                self.above_nmi.get(x / 4).copied()
            } else {
                None
            },
            left: if le {
                self.left_nmi.get(y / 4).copied()
            } else {
                None
            },
            up_available: up,
            left_available: le,
        }
    }

    /// Coefficient neighbor spans for a transform at (x, y) of w x h pixels,
    /// in 4x4 units, clipped to the frame like C svt_aom_get_txb_ctx.
    pub(crate) fn coeff_neighbors(&self, x: usize, y: usize, w: usize, h: usize) -> (&[u8], &[u8]) {
        let x4 = x / 4;
        let y4 = y / 4;
        let w4 = (w / 4).min(self.above_coeff.len().saturating_sub(x4));
        let h4 = (h / 4).min(self.left_coeff.len().saturating_sub(y4));
        (
            &self.above_coeff[x4..x4 + w4],
            &self.left_coeff[y4..y4 + h4],
        )
    }

    /// Record a coded transform block's `(dc_sign << 6) | cul_level` byte
    /// over its 4x4 span (C: neighbor array unit write after
    /// av1_write_coeffs_txb_1d).
    pub(crate) fn record_coeff(&mut self, x: usize, y: usize, w: usize, h: usize, val: u8) {
        let x4 = x / 4;
        let y4 = y / 4;
        for i in x4..(x4 + w / 4).min(self.above_coeff.len()) {
            self.above_coeff[i] = val;
        }
        for i in y4..(y4 + h / 4).min(self.left_coeff.len()) {
            self.left_coeff[i] = val;
        }
    }

    /// Chroma-plane coefficient neighbor spans for a transform at chroma
    /// coords (cx, cy) of cw x ch chroma pixels, in chroma 4x4 units,
    /// clipped to the plane like the luma variant. `uv`: 0 = U, 1 = V.
    pub(crate) fn coeff_neighbors_uv(
        &self,
        uv: usize,
        cx: usize,
        cy: usize,
        cw: usize,
        ch: usize,
    ) -> (&[u8], &[u8]) {
        let x4 = cx / 4;
        let y4 = cy / 4;
        let w4 = (cw / 4).min(self.above_coeff_uv[uv].len().saturating_sub(x4));
        let h4 = (ch / 4).min(self.left_coeff_uv[uv].len().saturating_sub(y4));
        (
            &self.above_coeff_uv[uv][x4..x4 + w4],
            &self.left_coeff_uv[uv][y4..y4 + h4],
        )
    }

    /// Record a chroma transform block's neighbor byte over its chroma
    /// 4x4 span (per-plane, like the decoder's per-plane entropy contexts).
    pub(crate) fn record_coeff_uv(
        &mut self,
        uv: usize,
        cx: usize,
        cy: usize,
        cw: usize,
        ch: usize,
        val: u8,
    ) {
        let x4 = cx / 4;
        let y4 = cy / 4;
        for i in x4..(x4 + cw / 4).min(self.above_coeff_uv[uv].len()) {
            self.above_coeff_uv[uv][i] = val;
        }
        for i in y4..(y4 + ch / 4).min(self.left_coeff_uv[uv].len()) {
            self.left_coeff_uv[uv][i] = val;
        }
    }

    /// Reset left context at the start of each SB row.
    /// In rav1d, `t.l` is reset per tile row (= SB row for single-tile).
    pub(crate) fn reset_left_for_sb_row(&mut self) {
        self.left_partition.fill(0);
    }

    /// Convert block width to our bsl (block size level).
    ///
    /// Task #91: the `_ => 3` catch-all used to fold 128 into the 64 level,
    /// which capped `partition_ctx` at ctx 15 and made the ctx 16..19 rows
    /// — the ONLY rows whose alphabet is the 8-symbol 128 set (C
    /// `svt_aom_partition_cdf_length`, entropy_coding.c:922) — unreachable
    /// dead code. A 128-wide node would have coded its partition symbol
    /// against the 64x64 CDF row with a 10-symbol alphabet: wrong
    /// probabilities AND wrong alphabet length. Byte-neutral at SB64
    /// (no node is ever 128 wide there).
    pub(super) fn bsl(width: usize) -> usize {
        match width {
            w if w <= 8 => 0,
            w if w <= 16 => 1,
            w if w <= 32 => 2,
            w if w <= 64 => 3,
            _ => 4,
        }
    }

    /// Convert our bsl to rav1d BlockLevel.
    /// bsl=0 (8x8) → bl=4, bsl=1 (16x16) → bl=3, bsl=2 (32x32) → bl=2,
    /// bsl=3 (64x64) → bl=1, bsl=4 (128x128) → bl=0 (BL_128X128).
    pub(super) fn bsl_to_block_level(bsl: usize) -> usize {
        4 - bsl
    }

    /// Raw neighbour bytes for NSQDBG dumps — `partition_sub`'s inputs
    /// without the bit extraction.
    #[cfg(feature = "std")]
    pub(crate) fn part_ctx_bytes(&self, x: usize, y: usize) -> (u8, u8) {
        (self.above_partition[x / 8], self.left_partition[y / 8])
    }

    /// Compute partition context (sub, 0-3) from tracked above/left values.
    /// Uses the same bit-extraction logic as rav1d's `get_partition_ctx`.
    pub(super) fn partition_sub(&self, x: usize, y: usize, bsl: usize) -> usize {
        let xb8 = x / 8;
        let yb8 = y / 8;
        let above_val = if xb8 < self.above_partition.len() {
            self.above_partition[xb8]
        } else {
            0
        };
        let left_val = if yb8 < self.left_partition.len() {
            self.left_partition[yb8]
        } else {
            0
        };
        // Extract bit at position bsl (matching rav1d's (4 - bl) = bsl)
        let above_bit = ((above_val >> bsl) & 1) as usize;
        let left_bit = ((left_val >> bsl) & 1) as usize;
        above_bit + 2 * left_bit
    }

    /// Get the partition context (ctx, nsymbs) for a block at (x, y) with given width.
    pub(crate) fn partition_ctx(&self, x: usize, y: usize, width: usize) -> (usize, usize) {
        let bsl = Self::bsl(width);
        let sub = self.partition_sub(x, y, bsl);
        let ctx = bsl * 4 + sub;
        // C `svt_aom_partition_cdf_length` (entropy_coding.c:922-930):
        // 4 at 8x8 (ctx 0..3 — only NONE/H/V/SPLIT fit), 8 at 128x128
        // (ctx 16..19 — EXT minus the geometrically impossible H4/V4),
        // 10 everywhere between. Cross-checked against
        // `sb128_geom::partition_cdf_length`, which is keyed on the square
        // size rather than the ctx; the two must agree.
        let nsymbs = match ctx {
            0..=3 => 4,
            4..=15 => 10,
            _ => 8,
        };
        (
            ctx.min(crate::entropy::context::PARTITION_CONTEXTS - 1),
            nsymbs,
        )
    }

    /// Update partition context after encoding a non-SPLIT partition.
    /// For SPLIT, the children update the context — don't call this for SPLIT.
    /// MD leaf commit: C `mode_decision_update_neighbor_arrays` writes
    /// `partition_context_lookup[bsize]` over the block span
    /// (product_coding_loop.c:179-192). For RECT leaves the above byte is
    /// the WIDTH's NONE row and the left byte the HEIGHT's — i.e. the
    /// per-dimension levels, not max(w, h) for both.
    pub(crate) fn update_partition_ctx_leaf(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
    ) {
        // C partition_context_lookup[bsize].above/.left — a pure function
        // of the corresponding DIMENSION (the AL_PART_CTX NONE columns
        // extended by the 4px value 0x1f). Sub-8 dims write the covering
        // 8x8 cell (both siblings write the same byte, matching C's
        // 4x4-granular arrays on readback).
        fn dim_byte(dim: usize) -> u8 {
            match dim {
                4 => 0x1f,
                8 => 0x1e,
                16 => 0x1c,
                32 => 0x18,
                64 => 0x10,
                _ => 0x00, // 128
            }
        }
        let above_val = dim_byte(width);
        let left_val = dim_byte(height);
        let xb8 = x / 8;
        let yb8 = y / 8;
        for i in xb8..(xb8 + (width / 8).max(1)).min(self.above_partition.len()) {
            self.above_partition[i] = above_val;
        }
        for i in yb8..(yb8 + (height / 8).max(1)).min(self.left_partition.len()) {
            self.left_partition[i] = left_val;
        }
    }

    pub(crate) fn update_partition_ctx(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        partition_type: crate::partition::PartitionType,
    ) {
        let bsl = Self::bsl(width.max(height));
        let bl = Self::bsl_to_block_level(bsl);
        let pt = partition_type as usize;
        if pt >= 10 || bl >= 5 {
            return;
        }
        let above_val = AL_PART_CTX[0][bl][pt];
        let left_val = AL_PART_CTX[1][bl][pt];
        // 0xff means invalid (SPLIT) — don't update
        if above_val == 0xff || left_val == 0xff {
            return;
        }
        let hsz_8 = width / 8; // half-size in 8x8 units = width/8
        let xb8 = x / 8;
        let yb8 = y / 8;
        for i in xb8..(xb8 + hsz_8).min(self.above_partition.len()) {
            self.above_partition[i] = above_val;
        }
        let vsz_8 = height / 8;
        for i in yb8..(yb8 + vsz_8).min(self.left_partition.len()) {
            self.left_partition[i] = left_val;
        }
    }

    /// Record a block's mode and skip status in the context maps.
    pub(crate) fn record_block(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        mode: u8,
        uv_mode: u8,
        skip: bool,
    ) {
        let x4 = x / 4;
        let y4 = y / 4;
        let w4 = w / 4;
        let h4 = h / 4;
        // Fill above row with this block's mode
        for i in x4..(x4 + w4).min(self.above_mode.len()) {
            self.above_mode[i] = mode;
            self.above_skip[i] = skip;
        }
        // Fill left column with this block's mode
        for i in y4..(y4 + h4).min(self.left_mode.len()) {
            self.left_mode[i] = mode;
            self.left_skip[i] = skip;
        }
        // Keep chroma ownership separately from the luma mode maps. Three
        // luma-only children of a split 8x8 must not overwrite the previous
        // group's coded UV mode while the fourth child is predicting chroma.
        // C `is_chroma_reference` (common_utils.h:315), generalized by
        // `ss_x`/`ss_y`: at 4:4:4 the `ss == 0` arms make EVERY block a
        // chroma reference and the unit masks are 0 — the stamp covers
        // exactly the block's own span.
        let chroma_ref = (x4 & 1 != 0 || w4 & 1 == 0 || self.ss_x == 0)
            && (y4 & 1 != 0 || h4 & 1 == 0 || self.ss_y == 0);
        if chroma_ref {
            let mx = (1usize << self.ss_x) - 1;
            let my = (1usize << self.ss_y) - 1;
            let cx = x4 & !mx;
            let cy = y4 & !my;
            for i in cx..(cx + w4.max(1 << self.ss_x)).min(self.above_uv_mode.len()) {
                self.above_uv_mode[i] = uv_mode;
            }
            for i in cy..(cy + h4.max(1 << self.ss_y)).min(self.left_uv_mode.len()) {
                self.left_uv_mode[i] = uv_mode;
            }
        }
    }

    /// Stamp one block's span of the inter mi grid — C's
    /// `svt_aom_update_mi_map` (product_coding_loop.c:670) restricted to the
    /// fields [`crate::port_entropy_inter`]'s context functions read.
    pub(crate) fn record_inter_mi(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        mi: crate::port_entropy_inter::NeighborMi,
        mv: [svtav1_types::motion::Mv; 2],
        partition_type: u8,
    ) {
        let (x4, y4) = (x / 4, y / 4);
        for i in x4..(x4 + w / 4).min(self.above_nmi.len()) {
            self.above_nmi[i] = mi;
        }
        for i in y4..(y4 + h / 4).min(self.left_nmi.len()) {
            self.left_nmi[i] = mi;
        }
        // ...and the FULL grid the MVP walk scans, when this frame has one.
        // C stamps both from the same `svt_aom_update_mi_map` call, so they
        // can never disagree here either.
        if let Some(env) = self.mvp_env.as_ref() {
            let e = crate::intrabc_mvp::MvpMiEntry {
                bsize: mi.bsize,
                mode: mi.mode,
                use_intrabc: mi.use_intrabc,
                ref_frame: mi.ref_frame,
                mv,
                partition: partition_type,
                interp_filters: mi.interp_filters,
                skip_mode: mi.skip_mode,
                skip: mi.skip,
                comp_group_idx: mi.comp_group_idx,
                compound_idx: mi.compound_idx,
            };
            let stride = env.mi_stride as usize;
            for r in y4..(y4 + h / 4).min(env.mi_rows as usize) {
                for c in x4..(x4 + w / 4).min(env.mi_cols as usize) {
                    self.mvp_grid[r * stride + c] = e;
                }
            }
        }
    }

    /// Record a block's luma palette (C's `mbmi->palette_mode_info`, read
    /// back by `svt_aom_get_palette_mode_ctx` / `svt_get_palette_cache_y`).
    /// `colors` is `None` for a non-palette block (palette_size 0 — every
    /// current leaf, until #71 chunk 3/4 injection wires a winning
    /// candidate through `BlockDecision.palette`). Stamped over the
    /// block's full mi span, exactly like [`Self::record_block`].
    pub(crate) fn record_palette(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        colors: Option<&[u16]>,
    ) {
        let x4 = x / 4;
        let y4 = y / 4;
        let w4 = w / 4;
        let h4 = h / 4;
        let n = colors.map_or(0, <[u16]>::len) as u8;
        debug_assert!((n as usize) <= svtav1_types::prediction::PALETTE_MAX_SIZE);
        let mut buf = [0u16; svtav1_types::prediction::PALETTE_MAX_SIZE];
        if let Some(c) = colors {
            buf[..c.len()].copy_from_slice(c);
        }
        for i in x4..(x4 + w4).min(self.above_palette.len()) {
            self.above_palette[i] = n;
            self.above_palette_colors[i] = buf;
        }
        for i in y4..(y4 + h4).min(self.left_palette.len()) {
            self.left_palette[i] = n;
            self.left_palette_colors[i] = buf;
        }
    }

    /// C `svt_aom_get_palette_mode_ctx` (entropy_coding.c:4240-4251): count
    /// of above/left neighbor blocks (when available — frame-edge gated,
    /// like every other above/left context lookup here) whose luma
    /// `palette_size > 0`. NO SB-row drop (unlike [`palette_cache`], which
    /// has C's `svt_get_palette_cache_y` above-row exception) — this reads
    /// the immediate neighbor exactly like `above_mode_ctx`/`left_mode_ctx`.
    pub(crate) fn palette_neighbor_ctx(&self, x: usize, y: usize) -> usize {
        let x4 = x / 4;
        let y4 = y / 4;
        let above = y > 0 && x4 < self.above_palette.len() && self.above_palette[x4] > 0;
        let left =
            x > self.tile_left_px && y4 < self.left_palette.len() && self.left_palette[y4] > 0;
        usize::from(above) + usize::from(left)
    }

    /// C `get_filt_type(xd, plane = 0)` (enc_intra_prediction.c:20): 1
    /// when the above OR left neighbour block's Y mode is smooth
    /// (SMOOTH/SMOOTH_V/SMOOTH_H), else 0. Neighbours are the blocks at
    /// (mi_row - 1, mi_col) / (mi_row, mi_col - 1); unavailable -> 0.
    pub(crate) fn filt_type_y(&self, x: usize, y: usize) -> i32 {
        let smooth = |m: u8| matches!(m, 9..=11);
        let ab = y > 0 && smooth(self.above_mode[x / 4]);
        let le = x > self.tile_left_px && smooth(self.left_mode[y / 4]);
        i32::from(ab || le)
    }

    /// C `get_filt_type(xd, plane > 0)` reads the CHROMA reference
    /// neighbours selected by `svt_aom_init_xd`: round to the 8x8 luma
    /// group, then choose its bottom-right 4x4 owner — `| (1<<ss)-1`
    /// picks that owner at 4:2:0 and is a no-op at 4:4:4 (ss == 0, the
    /// block IS its own chroma owner). An adjacent 4x4 luma-only block
    /// can carry a different, uncoded UV mode.
    pub(crate) fn filt_type_uv(&self, x: usize, y: usize) -> i32 {
        let smooth = |m: u8| matches!(m, 9..=11);
        let gy = 4usize << self.ss_y; // chroma-unit luma height (8 at 420)
        let gx = 4usize << self.ss_x; // chroma-unit luma width
        let ab = (y & !(gy - 1)) > self.tile_top_px
            && smooth(self.above_uv_mode[(x / 4) | ((1 << self.ss_x) - 1)]);
        let le = (x & !(gx - 1)) > self.tile_left_px
            && smooth(self.left_uv_mode[(y / 4) | ((1 << self.ss_y) - 1)]);
        i32::from(ab || le)
    }

    /// Get the above mode context at position (x, y) in pixel coordinates.
    pub(crate) fn above_mode_ctx(&self, x: usize) -> usize {
        let x4 = x / 4;
        let mode = if x4 < self.above_mode.len() {
            self.above_mode[x4]
        } else {
            0
        };
        crate::entropy::context::intra_mode_context(mode)
    }

    /// Get the left mode context at position (x, y) in pixel coordinates.
    pub(crate) fn left_mode_ctx(&self, y: usize) -> usize {
        let y4 = y / 4;
        let mode = if y4 < self.left_mode.len() {
            self.left_mode[y4]
        } else {
            0
        };
        crate::entropy::context::intra_mode_context(mode)
    }

    /// Get the skip context at position (x, y).
    pub(crate) fn skip_ctx(&self, x: usize, y: usize) -> usize {
        let x4 = x / 4;
        let y4 = y / 4;
        let above = x4 < self.above_skip.len() && self.above_skip[x4];
        let left = y4 < self.left_skip.len() && self.left_skip[y4];
        crate::entropy::context::get_skip_context(above, left)
    }

    /// tx_size context for a block at (x, y) of w x h pixels.
    ///
    /// C `get_tx_size_context(xd)` (entropy_coding.c:4642-4676):
    /// `above = above_txfm_context[0] >= tx_size_wide[max_tx_size]`,
    /// `left = left_txfm_context[0] >= tx_size_high[max_tx_size]`, each
    /// gated on availability; both available → sum, one → that one,
    /// none → 0. For every bsize <= 64x64 the largest TX has the block's
    /// own dims, so max_tx_wide/high == w/h. The C is_inter neighbor
    /// override (use the neighbor's BLOCK dims instead of its TX dims)
    /// can't fire here: tx_depth is only coded on key frames, where every
    /// neighbor is intra.
    pub(crate) fn tx_size_ctx(&self, x: usize, y: usize, w: usize, h: usize) -> usize {
        // Availability == C xd->up_available / left_available
        // (set_mi_row_col: mi_row/col > TILE start — task #86: `above_txfm`
        // is allocated frame-wide but reset fresh per tile, so a
        // never-written cell already reads 0 (`0 >= w` is false for any
        // w > 0), making `has_above` numerically inert at a tile's own
        // top row EITHER way; gating on `tile_top_px` here anyway keeps
        // this consistent with `extract_neighbors`/`PartitionSearchConfig
        // ::tile_top_px` rather than relying on that coincidence).
        let has_above = y > self.tile_top_px;
        let has_left = x > self.tile_left_px;
        // IBC chunk 9 (aom-rs Root 6): C substitutes an is_inter
        // neighbour's BLOCK dims for its TXFM-context byte
        // (get_tx_size_context, entropy_coding.c:4626-4637). IntraBC
        // blocks are the only inter-classified neighbours on this port;
        // `above_inter_bw`/`left_inter_bh` hold their block dims (0 =
        // intra neighbour, the plain txfm-ctx compare).
        let above = if self.above_inter_bw[x / 4] != 0 {
            (self.above_inter_bw[x / 4] as usize >= w) as usize
        } else {
            (self.above_txfm[x / 4] as usize >= w) as usize
        };
        let left = if self.left_inter_bh[y / 4] != 0 {
            (self.left_inter_bh[y / 4] as usize >= h) as usize
        } else {
            (self.left_txfm[y / 4] as usize >= h) as usize
        };
        match (has_above, has_left) {
            (true, true) => above + left,
            (true, false) => above,
            (false, true) => left,
            (false, false) => 0,
        }
    }

    /// IBC chunk 9: stamp the inter-neighbour dims state over a block's
    /// footprint — the coded BLOCK dims for an IntraBC block (u8-safe:
    /// block dims <= 128), 0 for every intra block.
    pub(crate) fn record_inter_dims(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        use_intrabc: bool,
    ) {
        let (bw, bh) = if use_intrabc {
            (w as u8, h as u8)
        } else {
            (0, 0)
        };
        let x4 = x / 4;
        let y4 = y / 4;
        for i in x4..(x4 + w / 4).min(self.above_inter_bw.len()) {
            self.above_inter_bw[i] = bw;
        }
        for i in y4..(y4 + h / 4).min(self.left_inter_bh.len()) {
            self.left_inter_bh[i] = bh;
        }
    }

    /// The frame height in pixels this context spans (the C
    /// `mb_to_bottom_edge` clip base for the var-tx walk).
    pub(crate) fn frame_h_px(&self) -> usize {
        self.left_txfm.len() * 4
    }

    /// Update the TXFM context arrays after coding a block.
    ///
    /// C `set_txfm_ctxs(tx_size, n8_w, n8_h, skip && is_inter, xd)`
    /// (entropy_coding.c:4614-4625): above cells over the block's mi
    /// columns take tx_size_wide, left cells over its mi rows take
    /// tx_size_high. Runs for EVERY block (both branches of
    /// av1_code_tx_size), signaling or not. Our blocks always use the
    /// full-block TX and the skip||inter override stores block dims —
    /// identical values here either way.
    /// C `set_txfm_ctxs(tx_size, n8_w, n8_h, 0, xd)` with an explicit
    /// CHOSEN tx size — above cells take tx_size_wide, left cells
    /// tx_size_high, over the block's mi span (entropy_coding.c:4614;
    /// MD mirror mode_decision_update_neighbor_arrays,
    /// product_coding_loop.c:246-256).
    pub(crate) fn record_txfm_dims(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        tx_w: usize,
        tx_h: usize,
    ) {
        let x4 = x / 4;
        let y4 = y / 4;
        for i in x4..(x4 + w / 4).min(self.above_txfm.len()) {
            self.above_txfm[i] = tx_w as u8;
        }
        for i in y4..(y4 + h / 4).min(self.left_txfm.len()) {
            self.left_txfm[i] = tx_h as u8;
        }
    }

    /// The block's above TXFM-context span (tx dims in px per 4x4 unit) —
    /// the seed of the inter var-tx walk's local copy (IBC chunk 7; C
    /// `svt_aom_get_tx_size_bits` memcpy, rd_cost.c:1790-1795).
    pub(crate) fn txfm_above_span(&self, x: usize, w: usize) -> &[u8] {
        let x4 = x / 4;
        &self.above_txfm[x4..(x4 + w / 4).min(self.above_txfm.len())]
    }

    /// The block's left TXFM-context span (IBC chunk 7).
    pub(crate) fn txfm_left_span(&self, y: usize, h: usize) -> &[u8] {
        let y4 = y / 4;
        &self.left_txfm[y4..(y4 + h / 4).min(self.left_txfm.len())]
    }

    /// The block's above coefficient-context byte span (4x4 units),
    /// clipped to the frame — the seed of the MD TX-local overlay
    /// (C tx_reset_neighbor_arrays copies the committed arrays).
    pub(crate) fn above_coeff_span(&self, x: usize, w: usize) -> &[u8] {
        let x4 = x / 4;
        &self.above_coeff[x4..(x4 + w / 4).min(self.above_coeff.len())]
    }

    /// The block's left coefficient-context byte span (4x4 units).
    pub(crate) fn left_coeff_span(&self, y: usize, h: usize) -> &[u8] {
        let y4 = y / 4;
        &self.left_coeff[y4..(y4 + h / 4).min(self.left_coeff.len())]
    }
}

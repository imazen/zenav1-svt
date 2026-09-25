use super::*;

pub(super) fn use_angle_delta(width: u16, height: u16) -> bool {
    !matches!((width, height), (4, 4) | (4, 8) | (8, 4))
}

/// `eob = 1 + max{ i : coeffs[scan[i]] != 0 }`, else 0 — the scan-order
/// end-of-block the coefficient writer codes, recovered from the finished
/// raster `coeffs` by walking the scan BACKWARDS and returning on the first
/// non-zero.
///
/// The reverse walk is not a style choice. The three pack sites below used to
/// carry the forward form, `for (i, &pos) in scan.iter().enumerate() { if
/// coeffs[pos] != 0 { eob = i + 1 } }`, which runs the whole transform block
/// and takes a roughly even data-dependent branch at every position:
/// `stall_attrib_2026-09-05` measured 316,928 simulated mispredicts at a
/// 17.16 % rate on that one line — 82 % of `encode_block_syntax`'s total and
/// 3.4 % of the photo_cid p6 frame's — against 3.10 % for the reverse form
/// already written at `quant.rs:318`. This is a private twin of that helper
/// rather than a call into it, deliberately: `quant::eob_from_qcoeff` is
/// inlined into `quantize_fp`/`quantize_fp_hbd`, which are hot at preset 2,
/// and adding three more call sites there would change that codegen for no
/// reason.
#[inline]
pub(super) fn eob_from_scan_rev(scan: &[u16], coeffs: &[i32]) -> i32 {
    for i in (0..scan.len()).rev() {
        if coeffs[scan[i] as usize] != 0 {
            return i as i32 + 1;
        }
    }
    0
}

/// Write one chroma plane's transform block (`uv`: 0 = U, 1 = V) with the
/// C-exact coefficient writer, using that plane's own neighbor context
/// arrays but the SHARED plane_type=1 CDF tables (AV1 PLANE_TYPES = 2:
/// U and V share tables, contexts stay per-plane — libaom keeps
/// pd->above/left_entropy_context per plane while indexing every CDF with
/// `plane_type = plane > 0`).
///
/// The chroma tx type is NOT signaled: the decoder derives it from UVMode
/// via Mode_To_Txfm (spec compute_tx_type, plane > 0 intra) —
/// UV_DC_PRED -> DCT_DCT, which also selects the default scan. The writer
/// only emits tx_type symbols for plane_type == 0.
#[allow(clippy::too_many_arguments)]
pub(super) fn write_chroma_txb(
    writer: &mut crate::entropy::writer::AomWriter,
    coeff_fc: &mut crate::entropy::coeff_c::CoeffFc,
    ectx: &mut EntropyCtx,
    uv: usize,
    cx: usize,
    cy: usize,
    cw: usize,
    ch: usize,
    qcoeffs: &[i32],
    base_q_idx: u8,
    uv_tx_type: usize,
    is_chroma_larger: bool,
) {
    use crate::entropy::coeff_c;
    let tx_size = coeff_c::tx_size_from_dims(cw, ch);
    let (above, left) = ectx.coeff_neighbors_uv(uv, cx, cy, cw, ch);
    // plane != 0: txb_skip_ctx = (above nonzero) + (left nonzero) + 7,
    // or +10 when the chroma plane block is larger than this txb
    // (C svt_aom_get_txb_ctx else-branch; libaom get_txb_ctx num_pels
    // comparison). The 4th arg is the luma-only fast-path flag, unused
    // for plane != 0.
    let (txb_skip_ctx, dc_sign_ctx) = coeff_c::get_txb_ctx(1, above, left, true, is_chroma_larger);
    // eob relative to the scan of the DERIVED chroma tx type (the decoder
    // computes it from UVMode via Mode_To_Txfm — spec compute_tx_type,
    // plane > 0 intra: UV_DC -> DCT_DCT, UV_V -> ADST_DCT,
    // UV_H -> DCT_ADST, UV_SMOOTH -> ADST_ADST; DCT-only above 16x16).
    let scan = crate::entropy::scan_tables::scan(
        tx_size,
        crate::entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[uv_tx_type] as usize,
    );
    // Reverse scan with early return — the CHROMA twin of the two luma sites
    // in `encode_block_syntax`. `stall_attrib_2026-09-05` §3 named only those
    // two; this one runs for U and V on every coded block and has the same
    // ~50/50 data-dependent branch over the whole block.
    let eob = eob_from_scan_rev(scan, qcoeffs);
    let cul_level = coeff_c::write_coeffs_txb_1d(
        coeff_fc,
        writer,
        tx_size,
        uv_tx_type,
        1, // plane_type: U and V both use the chroma tables
        txb_skip_ctx,
        dc_sign_ctx,
        qcoeffs,
        eob,
        0, // intra_dir: unused for plane_type != 0 (no tx_type signaling)
        base_q_idx,
        false,
        false, // is_inter: dead for plane_type != 0 (no tx_type symbol)
    );
    ectx.record_coeff_uv(uv, cx, cy, cw, ch, cul_level as u8);
}

/// Encode block syntax (skip, mode, coefficients) WITHOUT a partition symbol.
///
/// This is the core block encoding used by both PARTITION_NONE leaves and
/// HORZ/VERT children. In AV1, HORZ/VERT children are always leaf blocks
/// that the decoder reads directly — no partition symbol is expected for them.
/// IBC chunk 9: bridge the inter var-tx tx_size writer with the
/// EntropyCtx txfm spans (copied out to end the immutable borrow before
/// the CDF-adapting write).
#[allow(clippy::too_many_arguments)]
pub(super) fn writer_tx_size_vartx_bridge(
    writer: &mut crate::entropy::writer::AomWriter,
    frame_ctx: &mut crate::entropy::context::FrameContext,
    ectx: &EntropyCtx,
    block_x: usize,
    block_y: usize,
    w: usize,
    h: usize,
    depth: u8,
) {
    let above: alloc::vec::Vec<u8> = ectx.txfm_above_span(block_x, w).to_vec();
    let left: alloc::vec::Vec<u8> = ectx.txfm_left_span(block_y, h).to_vec();
    crate::vartx::write_tx_size_vartx(
        writer,
        frame_ctx,
        &above,
        &left,
        w,
        h,
        depth,
        block_y,
        ectx.frame_h_px(),
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn encode_block_syntax(
    decision: &crate::partition::BlockDecision,
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
    // Diagnostic (SVTAV1_TRACEMARK=1): a block-boundary marker written INTO
    // the symtrace op stream on stderr, so a first-diverging-op index maps
    // straight onto a coded block. Opt-in — off for every existing caller —
    // because that stream is parsed as data by identity_diff.py, which
    // ignores unknown `#` lines (as does any C-side counterpart marker).
    #[cfg(feature = "std")]
    if !recon_only && crate::dbgenv::tracemark() {
        std::eprintln!(
            "# BLK mi=({},{}) bsize={} ibc={}",
            block_y / 4,
            block_x / 4,
            crate::entropy::context::block_size_index(
                decision.width as usize,
                decision.height as usize
            ),
            u8::from(decision.use_intrabc),
        );
    }
    // Diagnostic (SVTAV1_PACKTREE=<path>): one line per coded leaf — the
    // port's FINAL tree, file-only (no stderr noise; token-frugal drills).
    // `off=` is the entropy writer's byte position at block entry, which maps
    // a byte-level OBU divergence (`cmp -l`) straight onto a coded block; the
    // companion `PDV` line carries the IntraBC DV + its predictor + the tx
    // type (all invisible in the PTREE row, and the first things to rule out
    // when an IBC block diverges).
    // tools/tree_diff.py joins it against the C-side CTREE dump (the
    // svt_aom_update_mi_map --wrap, valid at every preset) and prints only
    // the flips. Field domains mirror the C wrap: C BlockSize enum id via
    // block_size_index; fi 5 = none; uv 13 = CFL; skip is derived on the
    // diff side from yeob/ueob/veob (C dumps the all-plane skip bit).
    #[cfg(feature = "std")]
    if !recon_only && let Some(path) = crate::dbgenv::packtree() {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let (ueob, veob) = decision
                .chroma_dec
                .as_ref()
                .map(|c| (c.2, c.3))
                .unwrap_or((0, 0));
            let _ = writeln!(
                f,
                "PTREE mi=({},{}) off={} bsize={} part={} mode={} uv={} fi={} ady={} aduv={} txd={} yeob={} ueob={} veob={} cflidx={} cflsgn={} pal={} ibc={}",
                block_y / 4,
                block_x / 4,
                writer.bytes_written(),
                crate::entropy::context::block_size_index(
                    decision.width as usize,
                    decision.height as usize
                ),
                decision.partition_type as u8,
                decision.intra_mode,
                decision.uv_mode,
                decision.filter_intra_mode,
                decision.angle_delta,
                decision.uv_angle_delta,
                decision.tx_depth,
                decision.eob,
                ueob,
                veob,
                decision.cfl_alpha_idx,
                decision.cfl_alpha_signs,
                decision.palette.as_ref().map(|p| p.0.len()).unwrap_or(0),
                // IntraBC is invisible in this dump otherwise, which made it
                // impossible to assert that a screen-content gate cell actually
                // exercised IBC rather than merely enabling it.
                u8::from(decision.use_intrabc),
            );
            let _ = writeln!(
                f,
                "PDV mi=({},{}) dvr={} dvc={} dvrefr={} dvrefc={} txt={} inter={} mvr={} mvc={} rf={} rf1={} mode={} cg={} ci={} ct={} mm={}",
                block_y / 4,
                block_x / 4,
                decision.dv.y,
                decision.dv.x,
                decision.dv_ref.y,
                decision.dv_ref.x,
                decision.tx_type,
                // The INTER half of the committed decision, so a PTREE dump
                // can be joined against C's `SVT_CINTER_OUT` the way the
                // intra half already joins against `SVT_CTREE_OUT`. Without
                // it an inter leaf is indistinguishable from a DC intra one
                // in this dump (`mode` is `intra_mode`, which an inter block
                // leaves at 0) — which is exactly how a decode failure on an
                // inter frame had no per-block evidence behind it.
                u8::from(decision.is_inter),
                decision.inter.as_deref().map_or(0, |b| b.mv[0].y),
                decision.inter.as_deref().map_or(0, |b| b.mv[0].x),
                decision.inter.as_deref().map_or(0, |b| b.ref_frame[0]),
                decision.inter.as_deref().map_or(0, |b| b.ref_frame[1]),
                decision.inter.as_deref().map_or(0, |b| b.mode as u8),
                decision.inter.as_deref().map_or(0, |b| b.comp_group_idx),
                decision.inter.as_deref().map_or(0, |b| b.compound_idx),
                decision
                    .inter
                    .as_deref()
                    .map_or(0, |b| b.interinter_comp_type),
                decision.inter.as_deref().map_or(0, |b| b.motion_mode as u8),
            );
        }
    }
    // Diagnostic (SVTAV1_BLKMARK=1): the same identity, but on STDERR and
    // therefore INTERLEAVED with the `symtrace` op log — which is what turns
    // an "op index N diverges" verdict into "block mi=(r,c) diverges". The
    // file dump above cannot do that: it is a separate stream with no
    // ordering relation to the op trace. Emitted at the top of
    // `encode_block_syntax`, i.e. after the block's partition symbol and
    // before every one of its mode/coeff symbols.
    #[cfg(feature = "std")]
    if !recon_only && crate::dbgenv::blkmark() {
        if let Some(id) = decision.inter.as_deref() {
            // Inter leaves join to C's `CWIN poc=<p> blk=(<x>,<y>) mode=<m>
            // rf0=<r> rf1=<r> mv=(<x>,<y>) drl=<d> skip=<s> skm=<s> bhc=<b>
            // txdep=<t>` — same block origin in pixels, same field order.
            // `bhc` (block-has-coeff) is `eob != 0` here.
            std::eprintln!(
                "RWIN blk=({},{}) {}x{} mode={} rf0={} rf1={} mv=({},{}) mv1=({},{}) \
                 drl={} skip={} skm={} bhc={} txdep={}",
                block_x,
                block_y,
                decision.width,
                decision.height,
                id.mode as u8,
                id.ref_frame[0],
                id.ref_frame[1],
                id.mv[0].x,
                id.mv[0].y,
                id.mv[1].x,
                id.mv[1].y,
                id.drl_index,
                u8::from(decision.eob == 0),
                u8::from(id.skip_mode),
                u8::from(decision.eob != 0),
                decision.tx_depth,
            );
        }
        std::eprintln!(
            "W BLKMARK mi=({},{}) {}x{} mode={} uv={} pal={} ibc={}",
            block_y / 4,
            block_x / 4,
            decision.width,
            decision.height,
            decision.intra_mode,
            decision.uv_mode,
            decision.palette.as_ref().map(|p| p.0.len()).unwrap_or(0),
            u8::from(decision.use_intrabc),
        );
    }
    // Diagnostic (SVTAV1_PACKTREE_COEFF): the block's PACKED nonzero
    // luma+chroma levels as (raster_idx:level) pairs — the port counterpart
    // of the C QLEV/CCOEF wrap dumps (final coded levels). Two modes:
    //   * value contains a comma ("mi_row,mi_col") → pin ONE block, stderr.
    //   * value is a PATH (no comma) → append EVERY coded leaf to that file
    //     (coding order), for a whole-frame join vs the C SVT_QLEVELS_OUT
    //     dump. Backward-compatible: existing "r,c" callers are unchanged.
    #[cfg(feature = "std")]
    if !recon_only && let Some(xy) = crate::dbgenv::packtree_coeff() {
        let is_pin = xy.contains(',');
        let want: alloc::vec::Vec<usize> = xy
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        let pinned = is_pin && want.len() == 2 && want[0] == block_y / 4 && want[1] == block_x / 4;
        if pinned || !is_pin {
            let fmt_nz = |q: &[i32], cap: usize| -> alloc::string::String {
                let mut s = alloc::string::String::new();
                let mut n = 0;
                for (i, &v) in q.iter().enumerate() {
                    if v != 0 && n < cap {
                        if n > 0 {
                            s.push(',');
                        }
                        s.push_str(&alloc::format!("{i}:{v}"));
                        n += 1;
                    }
                }
                s
            };
            let (unz, vnz) = decision
                .chroma_dec
                .as_ref()
                .map(|c| (fmt_nz(&c.0, 1024), fmt_nz(&c.1, 1024)))
                .unwrap_or_default();
            let line = alloc::format!(
                "PCOEF mi=({},{}) yeob={} txt={} ynz=[{}] unz=[{}] vnz=[{}]",
                block_y / 4,
                block_x / 4,
                decision.eob,
                decision.tx_type,
                fmt_nz(&decision.qcoeffs, 1024),
                unz,
                vnz
            );
            if is_pin {
                eprintln!("{line}");
            } else {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(xy)
                {
                    let _ = writeln!(f, "{line}");
                }
            }
        }
    }
    // Diagnostic (SVTAV1_PART_DUMP): every coded leaf's geometry + skip, to
    // diff the partition tree against the C entropy coder. No output change.
    #[cfg(feature = "std")]
    if !recon_only && crate::dbgenv::part_dump() {
        eprintln!(
            "RSPART x{block_x} y{block_y} {}x{} skip={} ymode={} uvmode={} txd={}",
            decision.width,
            decision.height,
            decision.eob == 0,
            decision.intra_mode,
            decision.uv_mode,
            decision.tx_depth
        );
    }
    // 4:2:0: encode this block's chroma pair FIRST (prediction reads the
    // live chroma recon written by previous blocks in coding order). The
    // min-8x8 luma policy guarantees the chroma block is exactly
    // (w/2, h/2) >= 4x4 and every block is a chroma reference.
    // C `is_chroma_reference` (common_utils.h:315), generalized by the
    // frame's subsampling: a sub-8x8 block on a subsampled axis carries
    // chroma only for the bottom/right member of the shared group; at
    // 4:4:4 (`ss == 0`) the `!ss` arms degenerate the rule to ALWAYS true —
    // every block is its own chroma reference. Non-ref blocks code NO
    // chroma txbs and leave the chroma entropy contexts untouched (spec
    // residual(): the chroma loop is skipped entirely).
    let blk_has_uv = {
        let bw_mi = decision.width as usize / 4;
        let bh_mi = decision.height as usize / 4;
        ((block_y / 4) % 2 == 1 || bh_mi.is_multiple_of(2) || ectx.ss_y == 0)
            && ((block_x / 4) % 2 == 1 || bw_mi.is_multiple_of(2) || ectx.ss_x == 0)
    };
    // Task #86: chroma-plane tile-row origin (`>> ss` — exact halving at
    // 4:2:0, identity at 4:4:4). Copied out of `ectx` before the closure
    // below so the closure doesn't need to borrow `ectx` too.
    let chroma_tile_top = ectx.tile_top_px >> ectx.ss_y;
    let chroma_tile_left = ectx.tile_left_px >> ectx.ss_x; // task #96, same rule
    // The chroma plane's ALIGNED extent, for the reference-sample clamp
    // (`extract_neighbors_tiled`'s `plane_w`/`plane_h`). Same copy-out-of-ectx
    // reason as the tile origins above.
    let chroma_plane_w = ectx.aligned_w_px >> ectx.ss_x;
    let chroma_plane_h = ectx.aligned_h_px >> ectx.ss_y;
    let chroma_blocks = chroma.as_mut().filter(|_| blk_has_uv).map(|cp| {
        let (ss_x, ss_y) = (ectx.ss_x, ectx.ss_y);
        let (bw_px, bh_px) = (decision.width as usize, decision.height as usize);
        let (bw_mi, bh_mi) = (bw_px / 4, bh_px / 4);
        let (mi_row, mi_col) = (block_y / 4, block_x / 4);
        // Decoder `adj_row`/`adj_col` (decodeframe.c `set_mi_row_col`): a
        // sub-8x8 dimension at an odd mi position on a SUBSAMPLED axis
        // shifts the chroma group origin back one mi to the pair base.
        // At 4:4:4 `ss == 0` so the adjustment never fires — the chroma
        // block is the block itself.
        let adj_col = if ss_x != 0 && (mi_col & 1) != 0 && bw_mi == 1 {
            mi_col - 1
        } else {
            mi_col
        };
        let adj_row = if ss_y != 0 && (mi_row & 1) != 0 && bh_mi == 1 {
            mi_row - 1
        } else {
            mi_row
        };
        // Chroma plane block origin/dims in chroma px (C
        // `get_plane_block_size`, `bwidth_uv = MAX(4, w >> ss)`).
        let cx = (adj_col * 4) >> ss_x;
        let cy = (adj_row * 4) >> ss_y;
        let cw = (bw_px >> ss_x).max(4);
        let ch = (bh_px >> ss_y).max(4);
        if let Some((u_q, v_q, u_eob, v_eob, u_rec, v_rec)) = decision.chroma_dec.as_ref() {
            // Funnel-decided chroma (M6 leaf funnel): the decision phase
            // already predicted (per the decided uv_mode), quantized and
            // reconstructed both planes with the C MDS3 path — copy its
            // recon into the walk planes so the plane evolution is
            // byte-identical, and code the decided coefficients.
            // A straddling leaf's reconstruction retains its full transform
            // stride. Clip the destination to the aligned plane: copying the
            // right-edge padding would otherwise wrap into the next row.
            let copy_w = cw.min(chroma_plane_w.saturating_sub(cx));
            let copy_h = ch.min(chroma_plane_h.saturating_sub(cy));
            for r in 0..copy_h {
                let dst = (cy + r) * cp.stride + cx;
                cp.u_recon[dst..dst + copy_w].copy_from_slice(&u_rec[r * cw..r * cw + copy_w]);
                cp.v_recon[dst..dst + copy_w].copy_from_slice(&v_rec[r * cw..r * cw + copy_w]);
            }
            let (uq, vq) = if recon_only {
                // The coefficient vecs only feed `write_chroma_txb` below —
                // skipped in recon mode, so the clones are dead work.
                (Vec::new(), Vec::new())
            } else {
                (u_q.clone(), v_q.clone())
            };
            ChromaBlock {
                cx,
                cy,
                cw,
                ch,
                txw: cw,
                txh: ch,
                bw_units: cw / 4,
                bh_units: ch / 4,
                larger: false,
                u: alloc::vec![(uq, *u_eob)],
                v: alloc::vec![(vq, *v_eob)],
            }
        } else {
            // Decoder `residual()` order (av1 decodeframe.c): per plane the
            // chroma plane block is tiled by `av1_get_max_uv_txsize` TXBs
            // (TX_4X4 at coded_lossless), TXB start positions clipped to
            // the plane block's in-frame 4x4-unit span (`max_block_wide/
            // high` = `mb_to_edge >> (3 + ss)` folded in). Each TXB is
            // predicted from the already-reconstructed neighbors, quantized
            // and reconstructed inside this walk — a later TXB of the same
            // block reads the earlier ones' recon, exactly as the decoder.
            // Chroma TXB size: `av1_get_uv_tx_size` = adjusted max rect of
            // the plane block — except at coded_lossless, where the intra
            // residual loop's `av1_get_tx_size(plane>0)` preempts to
            // TX_4X4 for EVERY plane (decodeframe.c:941 -> blockd.h:1147;
            // the `max_uv_txsize` arm at :291 is the INTER path only).
            let uv_tx = if base_q_idx == 0 {
                crate::entropy::coeff_c::TX_4X4
            } else {
                crate::entropy::coeff_c::adjusted_tx_size(
                    crate::entropy::coeff_c::tx_size_from_dims(cw.min(64), ch.min(64)),
                )
            };
            let txw = crate::entropy::coeff_c::TX_SIZE_WIDE[uv_tx];
            let txh = crate::entropy::coeff_c::TX_SIZE_HIGH[uv_tx];
            // `mb_to_right/bottom_edge`: the luma block's overshoot of the
            // aligned (mi-padded) frame edge in 1/8 luma px — negative when
            // the block straddles.
            let edge_r = (ectx.aligned_w_px as i64 - (block_x + bw_px) as i64) * 8;
            let edge_b = (ectx.aligned_h_px as i64 - (block_y + bh_px) as i64) * 8;
            // `max_block_units_ss`: in-frame 4x4-unit span of the plane
            // block; TXB start positions beyond it are dropped.
            let units_w = ((cw as i64 + if edge_r < 0 { edge_r >> (3 + ss_x) } else { 0 }) >> 2)
                .max(0) as usize;
            let units_h = ((ch as i64 + if edge_b < 0 { edge_b >> (3 + ss_y) } else { 0 }) >> 2)
                .max(0) as usize;
            // A genuinely inter block codes no `uv_mode`: its chroma
            // prediction is the block's own motion compensation (the
            // decoder's inter chroma arm of `av1_inter_prediction`), so the
            // residual must be against that prediction — the intra-DC arm
            // below would reconstruct to pixels the decoder never produces.
            // Predict the whole plane block once per block; each TXB then
            // residual-codes against its own corner, exactly as the decoder
            // composes pred + inverse-quantized residual per TXB. An
            // all-zero residual reproduces the prediction — which IS a
            // skipped inter block's chroma recon.
            let inter_chroma_pred = if decision.is_inter {
                let ic = decision
                    .inter
                    .as_deref()
                    .expect("is_inter leaf without its InterDecision payload");
                let (uref, vref) = cp
                    .ref_uv
                    .expect("inter leaf on a frame with no chroma reference");
                let mut u_pred = alloc::vec![0u8; cw * ch];
                let mut v_pred = alloc::vec![0u8; cw * ch];
                crate::inter_pred_arm::predict_inter_chroma_whole(
                    uref,
                    vref,
                    block_x,
                    block_y,
                    bw_px,
                    bh_px,
                    ic.mv[0],
                    ic.interp_filters,
                    cp.sb_size,
                    cp.frame_w,
                    cp.frame_h,
                    &mut u_pred,
                    &mut v_pred,
                    cw,
                    ss_x,
                    ss_y,
                );
                Some((u_pred, v_pred))
            } else {
                None
            };
            // The decoder-derived chroma tx type — the same `uv_tt` the
            // emission arm below computes. Only covering-luma-txb index 0
            // matters here: a multi-TXB-covering decision (tx_depth > 0)
            // comes from the funnel, which always carries `chroma_dec`.
            let inter_uv_tt = if decision.is_inter {
                let (cover_eob, cover_tt) = if decision.tx_depth == 0 {
                    (decision.eob, decision.tx_type)
                } else {
                    (
                        decision.txb_eobs.first().copied().unwrap_or(0),
                        decision.txb_tx_types.first().copied().unwrap_or(0),
                    )
                };
                crate::leaf_funnel::inter_uv_tx_type(cover_eob, cover_tt, base_q_idx == 0, txw, txh)
            } else {
                0
            };
            let mut u_txbs = alloc::vec::Vec::new();
            let mut v_txbs = alloc::vec::Vec::new();
            for uv in 0..2usize {
                let (src, recon, qindex, qm) = if uv == 0 {
                    (cp.u_src, &mut *cp.u_recon, cp.qindex_u, cp.qm_u)
                } else {
                    (cp.v_src, &mut *cp.v_recon, cp.qindex_v, cp.qm_v)
                };
                let mut row = 0usize;
                while row < units_h {
                    let mut col = 0usize;
                    while col < units_w {
                        let (q, eob) = if let Some((u_pred, v_pred)) = &inter_chroma_pred {
                            let pred = if uv == 0 { u_pred } else { v_pred };
                            // `inter_uv_tx_type`'s usize is the C TxType
                            // index — `TxType`'s repr(u8) order.
                            let tt = match inter_uv_tt {
                                1 => svtav1_types::transform::TxType::AdstDct,
                                2 => svtav1_types::transform::TxType::DctAdst,
                                3 => svtav1_types::transform::TxType::AdstAdst,
                                9 => svtav1_types::transform::TxType::Idtx,
                                _ => svtav1_types::transform::TxType::DctDct,
                            };
                            crate::partition::encode_chroma_block_pred(
                                src,
                                recon,
                                cp.stride,
                                cx + col * 4,
                                cy + row * 4,
                                txw,
                                txh,
                                &pred[(row * 4) * cw + col * 4..],
                                cw,
                                qindex,
                                cp.c_quant,
                                qm,
                                tt,
                            )
                        } else {
                            crate::partition::encode_chroma_block_dc(
                                src,
                                recon,
                                cp.stride,
                                cx + col * 4,
                                cy + row * 4,
                                txw,
                                txh,
                                qindex,
                                cp.c_quant,
                                qm,
                                chroma_tile_top,
                                chroma_tile_left,
                                chroma_plane_w,
                                chroma_plane_h,
                            )
                        };
                        // The eob must survive recon mode: `skip` below is
                        // the signaled `skip_txfm` — it folds in chroma eobs —
                        // and the recon walk's copy feeds CDEF's skip-all
                        // dlist. Empting only the coeff vec (write_chroma_txb
                        // is unreachable here) mirrors the funnel arm above.
                        let q_ship = if recon_only { Vec::new() } else { q };
                        if uv == 0 {
                            u_txbs.push((q_ship, eob));
                        } else {
                            v_txbs.push((q_ship, eob));
                        }
                        col += txw / 4;
                    }
                    row += txh / 4;
                }
            }
            ChromaBlock {
                cx,
                cy,
                cw,
                ch,
                txw,
                txh,
                bw_units: units_w,
                bh_units: units_h,
                larger: cw * ch > txw * txh,
                u: u_txbs,
                v: v_txbs,
            }
        }
    });

    // The block-level skip flag means ALL planes are zero (the decoder
    // reads no txbs at all for skip blocks and zeroes every plane's
    // entropy context — spec reset_block_context / libaom
    // av1_reset_entropy_context). Per-plane eob==0 inside a non-skip
    // block is carried by that plane's own txb_skip symbol instead.
    let skip = decision.eob == 0
        && chroma_blocks
            .as_ref()
            .is_none_or(|cb| cb.u.iter().all(|t| t.1 == 0) && cb.v.iter().all(|t| t.1 == 0));
    if recon_only {
        // Recon-only walk: every symbol write, CDF update, context track and
        // coded-area sum below is walk-local state — the only survivors of a
        // walk whose bytes never ship are the chroma recon planes (written
        // above) and the deblock geometry. Record exactly what the tail of
        // this function records on the bit-producing walk and stop here.
        geom.record_block(
            block_x,
            block_y,
            decision.width as usize,
            decision.height as usize,
            decision.is_inter,
            skip,
        );
        let deblock_tx_is_block_max = decision.is_inter && skip;
        if decision.tx_depth > 0 && !deblock_tx_is_block_max {
            let (txw, txh) = crate::leaf_funnel::txb_dims_at_depth(
                decision.width as usize,
                decision.height as usize,
                decision.tx_depth,
            );
            let cols = decision.width as usize / txw;
            let txbs = cols * (decision.height as usize / txh);
            for txb in 0..txbs {
                geom.record_tx_dims(
                    block_x + (txb % cols) * txw,
                    block_y + (txb / cols) * txh,
                    txw,
                    txh,
                );
            }
        }
        return;
    }
    // C `update_b` (coding_loop.c:1605-1643), which runs on every coded block
    // of a `!scs->allintra` picture and is the SOURCE of the three coded-area
    // statistics the NEXT frame reads off this picture's reference object.
    // Placed here, immediately beside the `skip` the walk already computed,
    // because C's own skip accumulator is `blk_ptr->block_has_coeff == 0` and
    // a second derivation of that would diverge.
    if let Some(acc) = ectx.coded_area.as_mut() {
        acc.add_block(decision, block_x, block_y, skip);
    }
    // C `block_mi.skip_mode` — the winner's flag, set by `svt_aom_full_cost`'s
    // skip-mode arbitration. A skip-mode block codes ONE symbol here and the
    // whole mode-info group is suppressed below (`if (!skip_mode)`,
    // entropy_coding.c:5155).
    let skip_mode = decision.inter.as_deref().is_some_and(|b| b.skip_mode);
    // C `entropy_coding.c:5119`, in the non-intra-FRAME arm of
    // `write_modes_b`, immediately before `encode_skip_coeff_av1`:
    //
    // ```c
    // if (frm_hdr->skip_mode_params.skip_mode_flag && is_comp_ref_allowed(bsize))
    //     encode_skip_mode_av1(blk_ptr, frame_context, ec_writer, skip_mode);
    // if (!skip_mode) encode_skip_coeff_av1(...);
    // ```
    //
    // It is a FRAME-level arm, not a block-level one: an INTRA block of an
    // inter frame gets the symbol too. C codes the SYMBOL either way, and
    // omitting it shifts every following bit of the block.
    if !is_key
        && let Some(st) = ectx.inter_syntax.as_ref()
        && st.skip_mode_flag
        && let Some(bsize) =
            svtav1_types::block::BlockSize::from_u8(crate::entropy::context::block_size_index(
                decision.width as usize,
                decision.height as usize,
            ) as u8)
        && crate::port_entropy_inter::refframe::is_comp_ref_allowed(bsize)
    {
        let nb = ectx.inter_neighbors(block_x, block_y);
        crate::port_entropy_inter::modes::encode_skip_mode(
            writer,
            &mut frame_ctx.inter,
            &nb,
            skip_mode,
        );
    }
    // C `if (!skip_mode) encode_skip_coeff_av1(...)` (entropy_coding.c:5124)
    // — a skip-mode block does NOT carry the normal skip-coefficient symbol;
    // skip mode IS its skipped-residual path.
    if !skip_mode {
        let skip_ctx = ectx.skip_ctx(block_x, block_y);
        #[cfg(feature = "std")]
        if crate::dbgenv::skdbg() {
            std::eprintln!(
                "SKDBG org=({},{}) {}x{} skip={}",
                block_x,
                block_y,
                decision.width,
                decision.height,
                skip as u8
            );
        }
        crate::entropy::context::write_skip(writer, frame_ctx, skip_ctx, skip);
    }

    // cdef_idx (C write_cdef, entropy_coding.c:3986-4017; spec read_cdef):
    // at the FIRST NON-SKIP coded block of each 64x64 FILTER BLOCK,
    // `cdef_bits` raw literal bits carry that filter block's strength
    // index. Armed by the walk at SB start only when cdef_bits > 0
    // (aom_write_literal with 0 bits is a no-iteration loop).
    //
    // The filter block is 64x64 ALWAYS, so an SB128 superblock emits up to
    // FOUR literals — C's `cdef_transmitted[4]` latch, indexed
    // `!!(mi_col & 16) + 2 * !!(mi_row & 16)` (mi 16 == 64 px). Emitting
    // just one per SB (the pre-SB128 model) leaves the decoder expecting
    // literals the encoder never wrote: a CORRUPT tile, not merely a
    // mismatched one. At SB64 `index` is always 0 and this is bit-for-bit
    // the previous behaviour.
    // C `write_cdef(..., skip_mode ? 1 : skip_coeff, ...)` — a skip-mode
    // block counts as skipped for the cdef latch even though the
    // skip-coeff symbol was suppressed.
    if !(skip || skip_mode)
        && let Some(st) = ectx.cdef_sb.as_mut()
    {
        let index = if st.sb128 {
            ((block_x >> 6) & 1) + 2 * ((block_y >> 6) & 1)
        } else {
            0
        };
        if !st.transmitted[index] {
            st.transmitted[index] = true;
            writer.write_literal(u32::from(st.strengths[index]), u32::from(st.bits));
        }
    }

    // [SVT_HDR_MODE] per-SB delta-q (C entropy_coding.c:4997, spec 5.11.41
    // mode_info -> read_delta_qindex): only at the SB's upper-left block,
    // and only when (bsize != sb_size || !skip). C tests the block origin
    // against `sb_mi_size` (sb_size in MI units) — the REAL sb grid — so at
    // SB128 the symbol fires once per 128x128 SB. Emitting it at every
    // 64-boundary writes up to 4 extra symbols per SB: the decoder reads
    // one and desyncs the rest of the tile ("Failed to decode tile data").
    if let Some((res, prev, sb_sz)) = ectx.delta_q_state {
        let super_block_upper_left = block_x.is_multiple_of(sb_sz) && block_y.is_multiple_of(sb_sz);
        let is_sb_sized = decision.width as usize == sb_sz && decision.height as usize == sb_sz;
        if super_block_upper_left && (!is_sb_sized || !skip) {
            let cur = ectx.delta_q_sb_qindex;
            let reduced = (cur - prev) / i32::from(res);
            crate::entropy::mv_coding::write_delta_q_index(
                writer,
                &mut frame_ctx.delta_q_cdf,
                reduced,
            );
            ectx.delta_q_state = Some((res, cur, sb_sz));
            // aom `--delta-lf-mode` (spec 5.11.41 tail): nested in the SAME
            // SB-origin gate, immediately after the delta-qindex — one
            // reduced delta_lf symbol (single, not multi) over
            // `delta_lf_cdf`, value (cur_lf - prev_lf) / delta_lf_res.
            if let Some(prev_lf) = ectx.delta_lf_state {
                let cur_lf = ectx.delta_lf_sb;
                let reduced_lf = (cur_lf - prev_lf) / 2; // delta_lf_res = 2
                crate::entropy::mv_coding::write_delta_q_index(
                    writer,
                    &mut frame_ctx.delta_lf_cdf,
                    reduced_lf,
                );
                ectx.delta_lf_state = Some(cur_lf);
            }
        }
    }

    // use_intrabc flag (C write_modes_b -> write_intrabc_info,
    // entropy_coding.c:5021-5023 / :4405-4416, gated svt_aom_allow_intrabc;
    // spec intra_frame_mode_info): on an IBC frame the flag is coded for
    // EVERY block — the port codes use_intrabc = 0 until the DV search +
    // injection land (map chunks 5-9); the write adapts intrabc_cdf exactly
    // like C's aom_write_symbol, and the funnel chain sim shares this path
    // (the C MD-side twin: update_stats -> update_cdf(intrabc_cdf),
    // md_rate_estimation.c:854-855). Without the flag the FH's
    // allow_intrabc = 1 promises a symbol the tile lacks — an UNDECODABLE
    // stream, not merely a divergent one (aomdec outputs zero frames).
    // IBC chunk 9: the winner's real use_intrabc + DV. write_intrabc_info
    // codes the flag over intrabc_cdf (adapting) and, when set, the DV
    // diff vs dv_ref over ndvc at MV_SUBPEL_NONE (svt_av1_encode_dv).
    let use_intrabc = decision.use_intrabc;
    if is_key && ectx.allow_intrabc {
        crate::intrabc::write_intrabc_info(
            writer,
            &mut frame_ctx.intrabc_cdf,
            &mut frame_ctx.ndvc,
            use_intrabc,
            decision.dv,
            decision.dv_ref,
        );
    }

    // Mode syntax is ALWAYS coded — the skip flag only gates residuals
    // (AV1 intra_frame_mode_info reads y_mode regardless of skip) — EXCEPT
    // for a skip-mode block, where C wraps the whole mode-info group in
    // `if (!skip_mode)` (entropy_coding.c:5155): no `write_is_inter`, no
    // reference frames, no mode, no MVs, no compound or interp symbols.
    if !skip_mode && !is_key {
        // C `write_is_inter` (entropy_coding.c:1147) takes
        // `svt_av1_get_intra_inter_context(xd)` — a FOUR-valued context off
        // the above/left neighbours' `is_inter_block`, not a constant.
        //
        // The port passed a hard-coded 0. That was inert while no inter
        // frame could reach the pack, and it is a DECODER DESYNC the moment
        // one does: the decoder computes the real context, reads a symbol
        // from a different CDF row, and every bit after it is misaligned.
        // FOUND by decoding the experimental 2-frame stream — `aomdec`
        // reported "Failed to decode tile data" on frame 1 and `dav1d`
        // "Invalid argument", with frame 0 decoding cleanly.
        //
        // `intra_inter_context` is already ported and tier-1 gated in
        // `port_entropy_inter`, and takes exactly the `Neighbors` the mi
        // grid now supplies.
        let ctx =
            crate::port_entropy_inter::intra_inter_context(&ectx.inter_neighbors(block_x, block_y));
        crate::entropy::context::write_intra_inter(writer, frame_ctx, ctx, decision.is_inter);
    }

    if skip_mode {
        // C `if (!skip_mode)` (entropy_coding.c:5155) wraps the ENTIRE
        // mode-info group: `write_is_inter` (handled above), the intra/inter
        // mode dispatch, chroma mode-info, palette and the residual call.
        // A skip-mode block emits none of them.
    } else if use_intrabc {
        // C write_modes_b :5024-5089: y_mode + angle + uv mode-info +
        // palette + filter_intra are ALL suppressed for an IntraBC block
        // (each writer is nested under `use_intrabc == 0`).
    } else if decision.is_inter {
        // THE PRE-CAMPAIGN HOMEGROWN INTER ARM. It is reachable on the
        // shipped path (the `SVTAV1_INTER_EXPERIMENTAL` floor lift of
        // 2026-09-15, and the bit-depth lift of 2026-09-18, removed the
        // inter refusals above), and it is NOT a bitstream: measured
        // 2026-09-01 on
        // `gradient 64x64 q40 p6 frames=2`, it commits 24 inter leaves and
        // for each one writes an MV and NOTHING ELSE. Four defects, named so
        // the chunk that replaces this line with
        // `port_entropy_inter::write_inter_mode_info` has the list:
        //
        // 1. **A FRESH `NmvContext` PER BLOCK.** `write_mv` builds
        //    `NmvContext::default()` on every call, so no MV symbol adapts
        //    the frame's context and no MV after the first is coded against
        //    the probabilities a decoder holds. C's `av1_encode_mv` takes the
        //    FRAME's single adapting `nmvc`. This is a decoder desync, not a
        //    size difference.
        // 2. **No `write_ref_frames`, no inter mode symbol, no DRL, no
        //    interp filter.** A decoder reading `is_inter = 1` reads all of
        //    those before the MV, so it consumes the MV's bits as a reference
        //    index.
        // 3. **`allow_hp = true` is hard-coded**, while this frame's header
        //    writes `allow_high_precision_mv = 0` (measured, §1r).
        // 4. **The MV is written raw, not as a difference from the MVP
        //    stack's predictor** — `inter_mvp.rs` is ported and unwired.
        //
        // Separately measured, and it is a MODE DECISION fact rather than a
        // syntax one: the homegrown ME lands on `mv.x = -22` eighth-pel on a
        // content translation of exactly 3 pixels, where C finds the integer
        // `-24`. A sub-pel refinement that prefers a fractional position over
        // an exact integer match is evidence about the ME, which chunk C4
        // replaces wholesale.
        //
        // REPLACED (docs/INTER-ENCODE-PLAN.md §1s item 7): the arm below is
        // the proven `port_entropy_inter::block::write_inter_mode_info`,
        // whose output is byte-identical to C's frame-1 tile when fed C's
        // measured decision (`inter_tile_byte_gate`). All four defects above
        // are gone by construction — the walk writes `write_ref_frames`, the
        // inter mode symbol, the DRL group and the interp filter in C's
        // order, differences the MV against `pred_mv`, and takes the FRAME's
        // single adapting `nmvc` at the header's own precision.
        let blk = decision.inter.as_deref().expect(
            "an is_inter block reached the pack with no InterModeInfo — the writer \
             REFUSES rather than falling back, because the fallback is not a decodable \
             bitstream (see BlockDecision::inter)",
        );
        let st = ectx
            .inter_syntax
            .as_ref()
            .expect("an inter block on a frame with no inter frame-syntax state");
        let frame = st.syntax();
        let nb = ectx.inter_neighbors(block_x, block_y);
        // `predmv` / `inter_mode_ctx` / `drl_ctx` are DERIVED from the
        // committed mode-info map here rather than carried from MD — see
        // `crate::partition::InterDecision`.
        let (pred_mv, inter_mode_ctx, drl, overlappable_neighbors, num_proj_ref) = ectx
            .inter_mvp_fields(
                block_x,
                block_y,
                decision.width as usize,
                decision.height as usize,
                blk,
            )
            .expect("an inter block on a frame with no MVP environment");
        let info = crate::port_entropy_inter::block::InterModeInfo {
            // `c_bsize_index` IS the C `BlockSize` enum order, which is the
            // discriminant `from_u8` decodes.
            bsize: svtav1_types::block::BlockSize::from_u8(crate::leaf_funnel::c_bsize_index(
                decision.width as usize,
                decision.height as usize,
            ) as u8)
            .expect("c_bsize_index yields a valid BlockSize discriminant"),
            mode: blk.mode,
            ref_frame: blk.ref_frame,
            mv: blk.mv,
            pred_mv,
            inter_mode_ctx,
            drl,
            interintra: blk.is_interintra_used.then_some(
                crate::port_entropy_inter::compound::InterIntraInfo {
                    mode: match blk.interintra_mode {
                        1 => crate::port_entropy_inter::compound::InterIntraMode::VPred,
                        2 => crate::port_entropy_inter::compound::InterIntraMode::HPred,
                        3 => crate::port_entropy_inter::compound::InterIntraMode::SmoothPred,
                        _ => crate::port_entropy_inter::compound::InterIntraMode::DcPred,
                    },
                    use_wedge: blk.use_wedge_interintra,
                    wedge_index: blk.interintra_wedge_index.max(0) as u8,
                },
            ),
            motion_mode: blk.motion_mode,
            num_proj_ref,
            overlappable_neighbors,
            // `write_inter_mode_info` step 9 is gated on `has_second_ref`;
            // a single-reference block takes `None` and skips the whole
            // compound group, exactly as C's gate does.
            compound: (blk.ref_frame[1] > 0).then_some({
                if blk.comp_group_idx == 0 {
                    crate::port_entropy_inter::compound::CompGroup::A {
                        compound_idx: blk.compound_idx != 0,
                    }
                } else {
                    crate::port_entropy_inter::compound::CompGroup::B(
                        crate::port_entropy_inter::compound::InterInterComp {
                            comp_type: match blk.interinter_comp_type {
                                3 => crate::port_entropy_inter::compound::CompoundType::Diffwtd,
                                _ => crate::port_entropy_inter::compound::CompoundType::Wedge,
                            },
                            wedge_index: blk.interinter_wedge_index.max(0) as u8,
                            wedge_sign: blk.interinter_wedge_sign,
                            mask_type: blk.interinter_mask_type,
                        },
                    )
                }
            }),
            interp_filters: blk.interp_filters,
            skip_mode: blk.skip_mode,
        };
        // `FrameContext` carries `inter` and `nmvc` inline while the writer
        // takes them as separate `&mut`s (C reaches all three through one
        // `fc` pointer). Split the copies out and write them BACK — a caller
        // that dropped them would silently code every following block
        // against unadapted inter CDFs.
        // Diagnostic (SVTAV1_INTERDBG=1): the per-block inter decision AS THE
        // WRITER SEES IT — including the three fields derived here rather
        // than carried from MD (`pred_mv`, `inter_mode_ctx`, `drl_ctx`) and
        // the neighbour pair their contexts read. Field-for-field the C
        // `SVT_CINTER_OUT` dump plus the neighbours, so a divergence can be
        // localized to a block and a field instead of to a byte offset.
        //
        // It exists because a decode failure on an inter frame had NO
        // per-block evidence behind it: `SVTAV1_PACKTREE` shows `intra_mode`,
        // which an inter block leaves at 0, so an inter leaf was
        // indistinguishable from a DC intra one.
        #[cfg(feature = "std")]
        if crate::dbgenv::interdbg() {
            std::eprintln!(
                "IDBG mi=({},{}) bs={:?} mode={:?} rf={:?} mm={:?} npr={} mv=({},{}) mv1=({},{}) pmv=({},{}) imc={} interp={:#x} drl={:?} skm={} ii={:?} cgrp={} ctype={} cwidx={} cws={} cmask={} nb_up={} nb_left={} nbA={:?} nbL={:?}",
                block_y / 4,
                block_x / 4,
                info.bsize,
                info.mode,
                info.ref_frame,
                info.motion_mode,
                info.num_proj_ref,
                info.mv[0].y,
                info.mv[0].x,
                info.mv[1].y,
                info.mv[1].x,
                info.pred_mv[0].y,
                info.pred_mv[0].x,
                info.inter_mode_ctx,
                info.interp_filters,
                info.drl,
                info.skip_mode,
                info.interintra,
                blk.comp_group_idx,
                blk.interinter_comp_type,
                blk.interinter_wedge_index,
                blk.interinter_wedge_sign,
                blk.interinter_mask_type,
                nb.up_available,
                nb.left_available,
                nb.above.map(|a| (a.mode, a.ref_frame, a.interp_filters)),
                nb.left.map(|a| (a.mode, a.ref_frame, a.interp_filters)),
            );
        }
        // Disjoint field borrows: the writer adapts `frame_ctx.inter` and
        // `frame_ctx.nmvc` IN PLACE — no per-block clone of either struct
        // (~1.2 KB round trip removed from every coded inter block).
        crate::port_entropy_inter::block::write_inter_mode_info(
            writer,
            &mut crate::port_entropy_inter::refframe::RefFrameCdfs {
                comp_inter_cdf: &mut frame_ctx.comp_inter_cdf,
                comp_ref_cdf: &mut frame_ctx.comp_ref_cdf,
                single_ref_cdf: &mut frame_ctx.single_ref_cdf,
            },
            &mut frame_ctx.inter,
            &mut frame_ctx.nmvc,
            &nb,
            &frame,
            &info,
        );
    } else if is_key {
        let above_ctx = ectx.above_mode_ctx(block_x);
        let left_ctx = ectx.left_mode_ctx(block_y);
        crate::entropy::context::write_intra_mode_kf(
            writer,
            frame_ctx,
            above_ctx,
            left_ctx,
            decision.intra_mode,
        );
        // C av1_use_angle_delta(bsize) is `bsize >= BLOCK_8X8` in ENUM order
        // (reconintra.h:59): only BLOCK_4X4/4X8/8X4 are excluded — the 4:1
        // rects BLOCK_4X16/16X4 (enum 16/17) DO signal angle_delta. The
        // decoder reads the symbol for every directional mode on those
        // blocks; omitting it desyncs the tile.
        if use_angle_delta(decision.width, decision.height)
            && crate::entropy::context::is_directional_mode(decision.intra_mode)
        {
            crate::entropy::context::write_angle_delta(
                writer,
                frame_ctx,
                decision.intra_mode,
                decision.angle_delta,
            );
        }
    } else {
        let bsize_group = crate::entropy::context::block_size_group(
            decision.width as usize,
            decision.height as usize,
        );
        crate::entropy::context::write_intra_mode_inter(
            writer,
            frame_ctx,
            bsize_group,
            decision.intra_mode,
        );
        if use_angle_delta(decision.width, decision.height)
            && crate::entropy::context::is_directional_mode(decision.intra_mode)
        {
            crate::entropy::context::write_angle_delta(
                writer,
                frame_ctx,
                decision.intra_mode,
                decision.angle_delta,
            );
        }
    }

    // 4:2:0 chroma mode syntax — read by the decoder right after y_mode +
    // angle_delta_y when `!monochrome && is_chroma_ref` (libaom
    // read_intra_frame_mode_info, decodemv.c:824-836):
    //   uv_mode: cdf [cfl_allowed][y_mode], 14 syms if CFL allowed else 13
    //   (read_intra_mode_uv, decodemv.c:140). We always code UV_DC_PRED
    //   (symbol 0). CFL alphas only follow UV_CFL_PRED; angle_delta_uv only
    //   follows directional UV modes — UV_DC triggers neither.
    // CFL allowed = LUMA block w <= 32 && h <= 32 (is_cfl_allowed,
    // blockd.h, non-lossless path).
    //
    // ...and NOT for an INTER block. In C the whole chroma mode-info slice
    // lives inside `write_modes_b`'s intra branch (entropy_coding.c:5199-5215)
    // — an inter block's chroma mode is implied by its motion, so no symbol is
    // coded. The port's gate was `chroma_blocks.is_some() && !use_intrabc`
    // with a `debug_assert!(!decision.is_inter, "420 path is key/intra only")`
    // recording the assumption; the assumption stopped holding the moment the
    // inter arm reached the pack, and `identity_run` builds RELEASE, where the
    // assert is compiled out — so the extra `uv_mode` symbol was written
    // SILENTLY. FOUND by decoding the experimental 2-frame stream (`aomdec`:
    // "Failed to decode tile data"), not by any byte count.
    if chroma_blocks.is_some() && !use_intrabc && !decision.is_inter {
        // is_cfl_allowed (cfl.h): non-lossless -> LUMA block <= 32x32;
        // lossless -> the chroma PLANE block must be exactly 4x4. At
        // 4:2:0 lossless that reduces to "luma leaf is 8x8" (the only
        // leaf lossless produces); at 4:4:4 it narrows the CDF row and
        // alphabet for every larger leaf.
        let cfl_allowed = if base_q_idx == 0 {
            chroma_blocks
                .as_ref()
                .is_some_and(|cb| cb.cw == 4 && cb.ch == 4)
        } else {
            decision.width <= 32 && decision.height <= 32
        };
        crate::entropy::context::write_uv_mode(
            writer,
            frame_ctx,
            cfl_allowed,
            decision.intra_mode,
            decision.uv_mode,
        );
        // CfL alphas follow a UV_CFL_PRED chroma mode (encode_intra_chroma_
        // mode_av1, entropy_coding.c:1181; decoder read_cfl_alphas). CFL is
        // never directional, so angle_delta_uv is skipped for it.
        if decision.uv_mode == crate::entropy::context::UV_CFL_PRED {
            crate::entropy::context::write_cfl_alphas(
                writer,
                frame_ctx,
                decision.cfl_alpha_idx,
                decision.cfl_alpha_signs,
            );
        }
        // angle_delta_uv follows directional UV modes on >= 8x8 blocks
        // (read_intra_frame_mode_info, decodemv.c:833) — nonzero only
        // when the M5 ind-uv search picked a delta'd uv mode.
        if use_angle_delta(decision.width, decision.height)
            && crate::entropy::context::is_directional_mode(decision.uv_mode)
        {
            crate::entropy::context::write_angle_delta(
                writer,
                frame_ctx,
                decision.uv_mode,
                decision.uv_angle_delta,
            );
        }
    }

    // Palette flags: C codes them between the chroma mode-info slice and
    // the filter_intra flag (write_palette_mode_info, gated at
    // entropy_coding.c:5026 on !use_intrabc && svt_aom_allow_palette).
    // `decision.palette` is None on every current leaf (candidate
    // injection — #71 chunks 3/4 — doesn't wire a winner into
    // BlockDecision yet), so today this always takes the `None` arm:
    // BIT-IDENTICAL to the former write_no_palette_flags (symbol-0 y/uv
    // flags; the CDF updates + per-SB avg chain still run, keeping the
    // arithmetic stream aligned with C on screen-content frames). Once a
    // winner is wired, the `Some` arm below activates with no further
    // pack changes needed.
    //
    // cache/found/out_of_cache live in this outer scope (not just the
    // `if allow_palette` block) so the PALETTE MAP TOKENS write further
    // below — coded after filter_intra, per C order — can reuse them.
    let mut pal_found: alloc::vec::Vec<bool> = alloc::vec::Vec::new();
    let mut pal_out: alloc::vec::Vec<u16> = alloc::vec::Vec::new();
    let mut pal_n_out = 0usize;
    if let Some((colors, _idx_map)) = decision.palette.as_ref() {
        let pal_cache = palette_cache(ectx, block_x, block_y);
        pal_found = alloc::vec![false; pal_cache.len()];
        pal_out = alloc::vec![0u16; colors.len()];
        pal_n_out =
            crate::palette::index_color_cache(&pal_cache, colors, &mut pal_found, &mut pal_out);
    }
    if !decision.is_inter
        && !use_intrabc // C :5026: palette mode-info suppressed for IntraBC
        && crate::entropy::context::allow_palette(
            ectx.allow_sct,
            decision.width as usize,
            decision.height as usize,
        )
    {
        let neighbor_ctx = ectx.palette_neighbor_ctx(block_x, block_y);
        let palette_arg = decision.palette.as_ref().map(|(colors, _idx_map)| {
            (
                colors.as_slice(),
                pal_found.as_slice(),
                &pal_out[..pal_n_out],
            )
        });
        crate::entropy::context::write_palette_mode_info(
            writer,
            frame_ctx,
            decision.width as usize,
            decision.height as usize,
            decision.intra_mode,
            decision.uv_mode,
            chroma_blocks.is_some(),
            neighbor_ctx,
            palette_arg,
            u32::from(ectx.bit_depth),
        );
    }

    // use_filter_intra flag — C writes it right after the uv/palette
    // syntax and BEFORE code_tx_size, for every intra block passing
    // svt_aom_filter_intra_allowed (mode_decision.c:107): SH filter_intra
    // level != 0, mode == DC_PRED, **palette_size == 0**, and
    // block_size_wide/high[bsize] <= 32. Write order: entropy_coding.c:5050
    // (the flag is coded right after write_palette_mode_info, :5039). The
    // palette_size==0 gate is LOAD-BEARING: C codes NO filter_intra flag for
    // a palette block (palette forces the mode + tx), so a palette block that
    // priced/coded the flag emits an EXTRA symbol the decoder never reads,
    // desyncing the whole tile. (This was latent while palette was never
    // picked — allow_screen_content_tools=0; it fires the moment a
    // screen-content frame wins a DC-mode <=32x32 palette block.) We never
    // PREDICT with filter-intra so the flag is always 0 when coded, but on a
    // non-palette DC block the symbol MUST be coded or the decoder desyncs.
    if ectx.seq_filter_intra
        && !decision.is_inter
        && !use_intrabc // C :5050 nests under use_intrabc == 0
        && decision.intra_mode == 0 // DC_PRED
        && decision.palette.is_none() // palette_size == 0 (mode_decision.c:107)
        && decision.width <= 32
        && decision.height <= 32
    {
        let bsize_idx = crate::entropy::context::block_size_index(
            decision.width as usize,
            decision.height as usize,
        );
        let used = decision.filter_intra_mode != 5;
        crate::entropy::context::write_use_filter_intra(writer, frame_ctx, bsize_idx, used);
        if used {
            crate::entropy::context::write_filter_intra_mode(
                writer,
                frame_ctx,
                decision.filter_intra_mode,
            );
        }
    }

    // PALETTE MAP TOKENS — C's plane loop (entropy_coding.c:5064-5089):
    // `for plane in 0..2 { if palette_size[plane] > 0 { tokenize +
    // pack_map_tokens } }`, coded right after filter_intra and BEFORE
    // code_tx_size. Chroma palette is dead (`palette_size[1]` hard-0 at
    // injection — see docs/palette-port-map.md), so only plane 0 (Y) ever
    // fires; gated directly on `decision.palette` rather than re-deriving
    // `allow_palette` (a palette winner can only exist where it already
    // held, matching C's implicit invariant `palette_size > 0 =>
    // svt_aom_allow_palette` held at injection).
    if let Some((colors, idx_map)) = decision.palette.as_ref() {
        let w = decision.width as usize;
        let h = decision.height as usize;
        // C tokenizes and packs the map over the part of the block INSIDE the
        // frame -- `rows_within_bounds` / `cols_within_bounds` from
        // `svt_aom_get_block_dimensions` (palette.c:217-245), derived from
        // `mb_to_bottom_edge` / `mb_to_right_edge`, which go negative exactly
        // when the block straddles the aligned extent. The map STRIDE stays the
        // full block width; only the traversal shrinks.
        //
        // Writing the full block instead emits color-index symbols for rows and
        // columns the decoder never reads, which desyncs the tile. That was
        // latent while nothing straddled: 64-aligned frames have no straddling
        // block, and before the edge-aware PD1 walk a partial SB never reached
        // palette at presets 0..5. Enabling it turned this into three
        // DECODE-FAILs (screen 56x56 / 120x120 / 65x257 at q20 p0).
        let rows = h.min(ectx.aligned_h_px.saturating_sub(block_y));
        let cols = w.min(ectx.aligned_w_px.saturating_sub(block_x));
        debug_assert!(
            rows >= 1 && cols >= 1,
            "a coded block always has at least one in-frame row and column"
        );
        crate::entropy::context::write_palette_map_tokens(
            writer,
            frame_ctx,
            idx_map,
            w,
            rows,
            cols,
            colors.len(),
        );
    }

    // tx_size syntax — C av1_code_tx_size (entropy_coding.c:4697) called
    // from write_modes_b right after the uv/palette/filter_intra syntax
    // and before the residuals. The symbol exists ONLY at TX_MODE_SELECT
    // (`ectx.tx_mode_select`, the bit `crate::txs_arm::tx_mode_select` put in
    // the FH): then every INTRA block with bsize > 4x4 codes a tx_depth
    // symbol (the ACTUAL `decision.tx_depth` from the funnel's TXS search —
    // 0/1/2, NOT hardcoded to largest), and skip only suppresses it for inter
    // blocks. At TX_MODE_LARGEST the decoder INFERS the size and NOTHING is
    // coded. The neighbor context update (set_txfm_ctxs) runs for EVERY
    // block, signaling or not.
    //
    // This gate read `is_key` until 2026-09-01. That was the allintra arm's
    // rule (it signals TX_MODE_SELECT unconditionally) applied to every
    // frame, so a VIDEO-mode key frame at preset >= 10 — where the video arm
    // signals TX_MODE_LARGEST because `txs_level == 0` — declared LARGEST in
    // the header and then wrote one `tx_size_cdf` symbol per block anyway.
    // MEASURED on `diag 64x64 q40 p11` video, frame 0: the op-trace differ
    // (tools/ctrace-linux + identity_diff.py) put the FIRST divergence at the
    // first coded block, `CDF nsyms=2 icdf=[12800]` — TX_SIZE_CDF[0][0] —
    // present in the port and absent in C, with every partition, mode, uv
    // mode, luma tx type, luma eob and luma level already identical.
    {
        let w = decision.width as usize;
        let h = decision.height as usize;
        let depth = decision.tx_depth;
        // C `av1_code_tx_size` picks its arm on `is_inter_block(mbmi)`,
        // which is `use_intrabc || ref_frame[0] > INTRA_FRAME`
        // (block_structures.h:119) — an IntraBC block AND a genuinely inter
        // one both take the var-tx arm. While IntraBC was the only
        // inter-CLASSIFIED block this pack could emit, `use_intrabc` alone
        // was the same predicate; wiring a real inter block
        // (docs/INTER-ENCODE-PLAN.md §1s item 7) made the two differ, and an
        // inter block fell into the INTRA arm and coded a `tx_size` depth
        // symbol C does not write. MEASURED on the reference cell: the tile
        // came out `94 9a 9e` against C's `94 9a b0`, one extra 3-symbol
        // write at the end (`tx_size_cdf[2]`), everything before it
        // symbol-for-symbol and range-for-range identical.
        if use_intrabc || decision.is_inter {
            // C av1_code_tx_size inter arm (entropy_coding.c:4658-4676):
            // TX_MODE_SELECT && block_signals_txsize && !(is_inter && skip)
            // -> the var-tx walk over txfm_partition_cdf; the skip arm
            // codes NOTHING and stamps the BLOCK dims (set_txfm_ctxs with
            // skip && is_inter).
            // Left un-De-Morgan'd (clippy <=1.89 nonminimal_bool suggests
            // `!(skip || w == 4 && h == 4)`; current stable does not): the form
            // mirrors C's `!(is_inter && skip)` cited two lines above.
            #[allow(clippy::nonminimal_bool)]
            // C av1_code_tx_size applies its lossless gate before selecting
            // the inter/IntraBC arm too: residual-bearing lossless IBC must
            // not write a transform-partition symbol.
            if ectx.tx_mode_select && base_q_idx > 0 && !skip && !(w == 4 && h == 4) {
                writer_tx_size_vartx_bridge(writer, frame_ctx, ectx, block_x, block_y, w, h, depth);
                let (txw, txh) = crate::leaf_funnel::txb_dims_at_depth(w, h, depth);
                ectx.record_txfm_dims(block_x, block_y, w, h, txw, txh);
            } else {
                // skip (or 4x4): context stamp only — block dims for the
                // skip-inter arm (C set_txfm_ctxs bw = n8_w * MI_SIZE).
                if skip {
                    ectx.record_txfm_dims(block_x, block_y, w, h, w, h);
                } else {
                    let (txw, txh) = crate::leaf_funnel::txb_dims_at_depth(w, h, depth);
                    ectx.record_txfm_dims(block_x, block_y, w, h, txw, txh);
                }
            }
        } else {
            // C `av1_code_tx_size` (entropy_coding.c:4650-4658): the symbol is
            // coded only at TX_MODE_SELECT on a block that signals txsize AND
            // `!svt_av1_is_lossless_segment` — a coded-lossless frame
            // (base_q_idx 0 on this port's mainline path) derives TxMode
            // ONLY_4X4 and codes no depth. The 4x4 grid is still recorded for
            // the neighbours' contexts.
            if ectx.tx_mode_select && !(w == 4 && h == 4) && base_q_idx > 0 {
                let ctx = ectx.tx_size_ctx(block_x, block_y, w, h);
                crate::entropy::context::write_tx_depth(
                    writer,
                    frame_ctx,
                    w,
                    h,
                    ctx,
                    depth as usize,
                );
            }
            // set_txfm_ctxs records the CHOSEN tx dims (the C
            // tx_depth_to_tx_size chain — rect blocks halve the LONG dim
            // first) — the next blocks' tx_size contexts read them.
            let (txw, txh) = crate::leaf_funnel::txb_dims_at_depth(w, h, depth);
            ectx.record_txfm_dims(block_x, block_y, w, h, txw, txh);
        }
    }

    if !skip {
        // Residual order per spec residual(): all of plane 0's txbs, then
        // plane 1 (U), then plane 2 (V) — one full-size txb per plane here
        // (libaom decode_token_recon_block intra loop,
        // decodeframe.c:936-960). A plane with eob == 0 inside a non-skip
        // block still writes its txb (as a txb_skip=1 symbol) — only the
        // block-level skip removes txbs entirely.
        //
        // C-exact coefficient coding (av1_write_coeffs_txb_1d port).
        // The block uses a single full-size transform (tx_depth 0), so
        // plane_bsize == txsize_to_bsize[tx_size] and the luma
        // txb_skip_ctx fast path applies; dc_sign_ctx comes from the
        // per-4x4 (dc_sign << 6 | cul_level) neighbor bytes like C.
        use crate::entropy::coeff_c;
        let w = decision.width as usize;
        let h = decision.height as usize;
        // C `av1_read_tx_type`/`av1_get_tx_type` (decodemv.c:637): the luma
        // tx_type CDF is indexed by the FILTER-INTRA-mapped intra dir for
        // filter-intra blocks (use_filter_intra), not the coded DC mode —
        // `fimode_to_intradir[filter_intra_mode]`. Using DC here selects a
        // different intra_ext_tx_cdf instance than the decoder, desyncing
        // the tile once a filter-intra block with a non-DC-mapped mode is
        // coded (M0 filter_intra level 1 injects all five fi modes).
        let tx_intra_dir = if decision.filter_intra_mode != 5 {
            crate::leaf_funnel::FIMODE_TO_INTRADIR[decision.filter_intra_mode as usize] as usize
        } else {
            decision.intra_mode as usize
        };
        if decision.tx_depth == 0 {
            let tx_size = coeff_c::tx_size_from_dims(w, h);
            let (above, left) = ectx.coeff_neighbors(block_x, block_y, w, h);
            let (txb_skip_ctx, dc_sign_ctx) = coeff_c::get_txb_ctx(0, above, left, true, false);
            // 64-dim transforms keep only the 32-capped low-frequency
            // quadrant; the C writer expects that quadrant packed at the
            // adjusted stride.
            let aw = coeff_c::txb_wide(tx_size);
            let ah = coeff_c::txb_high(tx_size);
            let packed;
            let coeffs: &[i32] = if aw == w && ah == h {
                &decision.qcoeffs
            } else {
                let mut v = alloc::vec![0i32; aw * ah];
                for r in 0..ah {
                    v[r * aw..r * aw + aw].copy_from_slice(&decision.qcoeffs[r * w..r * w + aw]);
                }
                packed = v;
                &packed
            };
            // The decision's eob was derived from the mode-decision scan;
            // the bitstream eob must be relative to the C scan order for
            // this (tx_size, tx_type).
            let tx_type = decision.tx_type as usize;
            let scan = crate::entropy::scan_tables::scan(
                tx_size,
                crate::entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[tx_type] as usize,
            );
            // Reverse-scan-and-return ([`eob_from_scan_rev`]), not the
            // forward full-block walk this line used to carry: the forward
            // form's ~50/50 data-dependent test was the port's single largest
            // mispredict site — 316,928 simulated mispredicts at 17.16 %, 82 %
            // of `encode_block_syntax`'s total (`stall_attrib_2026-09-05`).
            let eob = eob_from_scan_rev(scan, coeffs);
            // Diagnostic aid: SVTAV1_CODED_EOB=1 prints the TRUE coded
            // scan-order eob per depth-0 leaf (the tree dump's d.eob is a
            // raster-order artifact). No output change.
            #[cfg(feature = "std")]
            if crate::dbgenv::coded_eob() {
                let nz = coeffs.iter().filter(|&&c| c != 0).count();
                eprintln!("CODED x{block_x} y{block_y} {w}x{h} tx{tx_type} scan_eob={eob} nz={nz}");
            }
            let cul_level = coeff_c::write_coeffs_txb_1d(
                coeff_fc,
                writer,
                tx_size,
                tx_type,
                0,
                txb_skip_ctx,
                dc_sign_ctx,
                coeffs,
                eob,
                tx_intra_dir,
                base_q_idx,
                false,
                // `is_inter_block` = `use_intrabc || ref_frame[0] > INTRA_FRAME`
                // — the depth-0 twin of the depth>0 site below. This is the
                // FOURTH place the port spelled that predicate `use_intrabc`,
                // which was the same thing only while IntraBC was the one
                // inter-classified block the pack could emit.
                use_intrabc || decision.is_inter,
            );
            ectx.record_coeff(block_x, block_y, w, h, cul_level as u8);
        } else {
            // tx_depth > 0: the C tx grid at this depth
            // (tx_depth_to_tx_size / tx_blocks_per_depth, raster order —
            // spec residual() / C av1_write_coeffs_mb), each txb with its
            // own neighbor contexts and tx type; the per-txb contexts
            // read the bytes recorded by the previous txbs.
            let (txw, txh) = crate::leaf_funnel::txb_dims_at_depth(w, h, decision.tx_depth);
            let cols = w / txw;
            let txbs = cols * (h / txh);
            let tx_size = coeff_c::tx_size_from_dims(txw, txh);
            for txb in 0..txbs {
                // IntraBC (inter-classified) blocks write their residual
                // in the recursive var-tx z-order (C write_inter_txb_coeff
                // recursion == the decoder's read order == the search
                // walk's txb_org_inter); intra blocks in raster. At depth
                // <= 1 the two coincide; at depth 2 they differ on the
                // square/h-rect bsizes and a raster write self-desyncs.
                let (rel_x, rel_y) = if decision.use_intrabc || decision.is_inter {
                    crate::leaf_funnel::txb_org_inter(w, h, decision.tx_depth, txb)
                } else {
                    ((txb % cols) * txw, (txb / cols) * txh)
                };
                let tx_x = block_x + rel_x;
                let tx_y = block_y + rel_y;
                #[cfg(feature = "std")]
                if crate::dbgenv::packtxb() {
                    let nz = decision.txb_qcoeffs[txb]
                        .iter()
                        .filter(|&&v| v != 0)
                        .count();
                    let list: alloc::string::String = decision.txb_qcoeffs[txb]
                        .iter()
                        .enumerate()
                        .filter(|&(_, &v)| v != 0)
                        .map(|(i, &v)| alloc::format!("{i}:{v},"))
                        .collect();
                    eprintln!(
                        "PACKTXB blk=({block_x},{block_y}) {w}x{h} d={} ibc={} txb={txb} pos=({rel_x},{rel_y}) tt={} nz={nz} cf=[{list}]",
                        decision.tx_depth, decision.use_intrabc, decision.txb_tx_types[txb],
                    );
                }
                let (above, left) = ectx.coeff_neighbors(tx_x, tx_y, txw, txh);
                let (txb_skip_ctx, dc_sign_ctx) =
                    coeff_c::get_txb_ctx(0, above, left, false, false);
                let tx_type = decision.txb_tx_types[txb] as usize;
                let coeffs = &decision.txb_qcoeffs[txb];
                let scan = crate::entropy::scan_tables::scan(
                    tx_size,
                    crate::entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[tx_type] as usize,
                );
                // Reverse scan with early return — the depth>0 twin of the
                // depth-0 site above (`stall_attrib_2026-09-05` §3).
                let eob = eob_from_scan_rev(scan, coeffs);
                let cul_level = coeff_c::write_coeffs_txb_1d(
                    coeff_fc,
                    writer,
                    tx_size,
                    tx_type,
                    0,
                    txb_skip_ctx,
                    dc_sign_ctx,
                    coeffs,
                    eob,
                    tx_intra_dir,
                    base_q_idx,
                    false,
                    // C `get_ext_tx_set` / `av1_get_tx_type` select the
                    // tx-type CDF rows on `is_inter_block(mbmi)` =
                    // `use_intrabc || ref_frame[0] > INTRA_FRAME`
                    // (block_structures.h:119). The port tested `use_intrabc`
                    // alone, which was the same predicate while IntraBC was
                    // the only inter-classified block the pack could emit —
                    // the THIRD site with that bug (the other two are
                    // `av1_code_tx_size`'s arm and `record_inter_dims`).
                    use_intrabc || decision.is_inter,
                );
                ectx.record_coeff(tx_x, tx_y, txw, txh, cul_level as u8);
            }
        }

        // Chroma txbs: plane 1 (U) then plane 2 (V), raster over the
        // plane block's (frame-edge clipped) TXB grid — decoder
        // `residual()` order. One TXB when `txw x txh` covers the plane
        // block (all of 4:2:0 at sb<=64), multi-TXB when it exceeds
        // `max_uv_txsize` (4:4:4 blocks > 32, lossless > 4).
        if let Some(cb) = chroma_blocks.as_ref() {
            let (cw, ch, cx, cy) = (cb.cw, cb.ch, cb.cx, cb.cy);
            // IBC chunk 9: on an INTER-classified (IntraBC) block the
            // decoder DERIVES the chroma tx type from the co-located luma
            // type (av1_get_tx_type plane>0 inter arm) — the same
            // follows-luma rule MDS3 applied: luma txb-0's type when the
            // chroma inter ext set admits it, else DCT. Intra blocks keep
            // the uv-mode mapping.
            // ...and a genuinely INTER block takes the same arm: C's
            // predicate is `is_inter_block(mbmi)`, not `use_intrabc`. The
            // chroma tx type selects the SCAN ORDER, so an inter block that
            // fell into the uv-mode arm scanned its chroma levels in a
            // different order than the decoder — the SIXTH site of this
            // predicate confusion, and the one with the least visible
            // symptom, because chroma is derived and codes no tx-type symbol
            // of its own.
            let uv_tt = if use_intrabc || decision.is_inter {
                // The decoder's tx_type_map cell at the leaf origin holds
                // the covering luma txb's CODED type — DCT_DCT when that
                // txb was all-zero (`read_coeffs_txb`, decodetxb.c:148-154)
                // or the block is lossless. Using the raw decision tx_type
                // here scanned chroma levels in a different order than the
                // decoder applied them (vidyo1 256x256 q20 p10 f3: the
                // luma-eob-0 leaf at x176 y128 committed tx_type 3 while
                // the map read DCT_DCT).
                let (cover_eob, cover_tt) = if decision.tx_depth == 0 {
                    (decision.eob, decision.tx_type)
                } else {
                    (
                        decision.txb_eobs.first().copied().unwrap_or(0),
                        decision.txb_tx_types.first().copied().unwrap_or(0),
                    )
                };
                crate::leaf_funnel::inter_uv_tx_type(
                    cover_eob,
                    cover_tt,
                    base_q_idx == 0,
                    cb.txw,
                    cb.txh,
                )
            } else {
                crate::leaf_funnel::uv_tx_type(decision.uv_mode, cb.txw, cb.txh)
            };
            #[cfg(feature = "std")]
            if crate::dbgenv::coded_eob() {
                let uv_ts = crate::entropy::coeff_c::tx_size_from_dims(cb.txw, cb.txh);
                let sidx = crate::entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[uv_tt] as usize;
                let uv_scan = crate::entropy::scan_tables::scan(uv_ts, sidx);
                let eob_of = |q: &[i32]| {
                    let mut e = 0usize;
                    for (i, &p) in uv_scan.iter().enumerate() {
                        if q[p as usize] != 0 {
                            e = i + 1;
                        }
                    }
                    e
                };
                let sum_of = |q: &[i32]| q.iter().map(|c| c.unsigned_abs() as u64).sum::<u64>();
                eprintln!(
                    "CODEDUV x{block_x} y{block_y} cw{cw} ch{ch} ntxb={}+{} tt={uv_tt} u_eob={:?} v_eob={:?} u_sum={:?} v_sum={:?}",
                    cb.u.len(),
                    cb.v.len(),
                    cb.u.iter().map(|(q, _)| eob_of(q)).collect::<Vec<_>>(),
                    cb.v.iter().map(|(q, _)| eob_of(q)).collect::<Vec<_>>(),
                    cb.u.iter().map(|(q, _)| sum_of(q)).collect::<Vec<_>>(),
                    cb.v.iter().map(|(q, _)| sum_of(q)).collect::<Vec<_>>(),
                );
            }
            for (uv, txbs) in [&cb.u, &cb.v].into_iter().enumerate() {
                let mut it = txbs.iter();
                let mut row = 0usize;
                while row < cb.bh_units {
                    let mut col = 0usize;
                    while col < cb.bw_units {
                        let (q, _eob) = it.next().expect("emission grid == production grid");
                        write_chroma_txb(
                            writer,
                            coeff_fc,
                            ectx,
                            uv,
                            cx + col * 4,
                            cy + row * 4,
                            cb.txw,
                            cb.txh,
                            q,
                            base_q_idx,
                            uv_tt,
                            cb.larger,
                        );
                        col += cb.txw / 4;
                    }
                    row += cb.txh / 4;
                }
                debug_assert!(it.next().is_none());
            }
        }
    } else {
        // Skipped blocks contribute zero cul_level neighbors (C writes the
        // txb through the same path with eob == 0 -> cul 0). For skip the
        // decoder zeroes EVERY plane's entropy context over the block span
        // (spec reset_block_context; libaom av1_reset_entropy_context) —
        // mirror that for the chroma planes too.
        ectx.record_coeff(
            block_x,
            block_y,
            decision.width as usize,
            decision.height as usize,
            0,
        );
        if let Some(cb) = chroma_blocks.as_ref() {
            // reset_block_context covers the whole PLANE BLOCK span (not
            // per-txb) — `bw_uv x bh_uv`, clipped inside `record_coeff_uv`.
            ectx.record_coeff_uv(0, cb.cx, cb.cy, cb.cw, cb.ch, 0);
            ectx.record_coeff_uv(1, cb.cx, cb.cy, cb.cw, cb.ch, 0);
        }
    }

    // Update context maps for subsequent blocks. The y_mode is signaled
    // for skip blocks too, and the decoder records it in its above/left
    // mode contexts — so must we.
    let mode = decision.intra_mode;
    ectx.record_block(
        block_x,
        block_y,
        decision.width as usize,
        decision.height as usize,
        mode,
        decision.uv_mode,
        skip,
    );
    // The inter mi grid (docs/INTER-ENCODE-PLAN.md §1s item 2), stamped for
    // EVERY block — an intra block inside a P frame is a neighbour the inter
    // reference-count and mode contexts read, so stamping only inter blocks
    // would leave the previous block's ref_frame standing in for it.
    ectx.record_inter_mi(
        block_x,
        block_y,
        decision.width as usize,
        decision.height as usize,
        crate::port_entropy_inter::NeighborMi {
            // C `block_mi.mode` — the INTER mode (`NEWMV` = 16 and up) on an
            // inter block, not `intra_mode`, which an inter block leaves at
            // 0 (`DC_PRED`).
            //
            // This is load-bearing and silent: `inter_mvp::setup_ref_mv_list`
            // counts `have_newmv_in_inter_mode(entry.mode)` over the scanned
            // neighbours into `newmv_count`, which selects the `mode_context`
            // a decoder ALSO derives from its own reconstructed mi map. Stamp
            // `DC_PRED` for an inter neighbour and the two derivations part
            // company, the `newmv` symbol is coded from a different CDF row
            // than the one read, and the tile desyncs from that block on.
            mode: decision.inter.as_deref().map_or(mode, |b| b.mode as u8),
            ref_frame: decision.inter.as_deref().map_or([0, -1], |b| b.ref_frame),
            interp_filters: decision.inter.as_deref().map_or(0, |b| b.interp_filters),
            use_intrabc: decision.use_intrabc,
            skip_mode: decision.inter.as_deref().is_some_and(|b| b.skip_mode),
            // C `block_mi.skip` (`!block_has_coeff`) — `lpd1_should_perform_tx`
            // reads it off the left/above mi as `both_neighbors_skip`.
            skip: decision.eob == 0
                && decision
                    .chroma_dec
                    .as_ref()
                    .is_none_or(|(_, _, u, v, _, _)| *u == 0 && *v == 0),
            // C `block_mi.comp_group_idx` / `compound_idx`, stamped so the
            // NEXT compound block's `comp_group_idx_context` /
            // `comp_index_context` read the same neighbour fields the
            // decoder derives. A skip-mode block's cand is always
            // MD_COMP_AVG (0, 1) — the decoder's own inferred values.
            comp_group_idx: decision.inter.as_deref().map_or(0, |b| b.comp_group_idx),
            compound_idx: decision.inter.as_deref().map_or(0, |b| b.compound_idx),
            bsize: crate::entropy::context::block_size_index(
                decision.width as usize,
                decision.height as usize,
            ) as u8,
        },
        // C `block_mi.mv` — a DV for an IntraBC block, the inter MV for an
        // inter one, zero for a plain intra block.
        if decision.use_intrabc {
            [decision.dv, svtav1_types::motion::Mv::ZERO]
        } else {
            decision
                .inter
                .as_deref()
                .map_or([svtav1_types::motion::Mv::ZERO; 2], |b| b.mv)
        },
        decision.partition_type as u8,
    );
    // IBC chunk 9 (aom-rs Root 6): stamp the inter-neighbour override
    // state — get_tx_size_context substitutes an is_inter neighbour's
    // BLOCK dims for its TXFM-context byte (entropy_coding.c:4626-4637);
    // IntraBC blocks are the only inter-classified neighbours here.
    ectx.record_inter_dims(
        block_x,
        block_y,
        decision.width as usize,
        decision.height as usize,
        // Same `is_inter_block` predicate as the tx_size arm above: the
        // neighbour override is about INTER blocks, of which IntraBC is one
        // kind and a real inter block is the other.
        use_intrabc || decision.is_inter,
    );
    // Palette neighbor state (C mbmi->palette_mode_info, stamped for
    // EVERY block — palette or not, matching record_block above).
    ectx.record_palette(
        block_x,
        block_y,
        decision.width as usize,
        decision.height as usize,
        decision
            .palette
            .as_ref()
            .map(|(colors, _idx_map)| colors.as_slice()),
    );

    // Deblocking geometry: exactly what the decoder derives per mi from
    // the parsed block — dims (single TX per block), signaled skip, and
    // inter-ness (skip only suppresses deblocking for inter blocks).
    // The decoder's mi grid: BLOCK identity/dims (chroma TX + pu_edge
    // derive from these) + the LUMA TX grid (quartered at tx_depth 1 —
    // chroma never splits with luma tx_depth).
    //
    // `cdf_only` marks the funnel chain's context-evolution replay: it codes
    // the same symbols to mutate CDFs but every pixel/grid side effect lands
    // in a write-only `sim_geom` sink. Skipping the records here is pure
    // dead-work removal — nothing downstream of a cdf_only walk reads geom.
    if !writer.cdf_only {
        geom.record_block(
            block_x,
            block_y,
            decision.width as usize,
            decision.height as usize,
            decision.is_inter,
            skip,
        );
    }
    // A SKIP INTER block's tx_depth is meaningless to the DEBLOCK, and
    // applying it here was a decoder mismatch. libaom's `get_transform_size`
    // takes the per-TU var-tx size only for `is_inter_block(mbmi) &&
    // !mbmi->skip_txfm`; a skip inter block falls through to `mbmi->tx_size`,
    // which for that case is the block's MAX tx size — it codes no residual,
    // so the searched depth never reached the bitstream and a decoder cannot
    // know about it. `record_block` already stored the block dims, which are
    // that max size for every bsize this port codes, so the fix is to leave
    // them alone.
    //
    // MEASURED (vidyo3 256x256 q40 p6 frames=2, docs/IDENTITY-STATUS.md): the
    // failing edges were all `inter=1 yeob=0 txd=1` 16x16 blocks. The decoder
    // read tx 16 and filtered 14-tap; the port read tx 8 (depth 1) and
    // filtered 8-tap, so encoder and decoder reconstructions disagreed by +-1
    // over the 14-tap footprint. An INTRA block never takes this branch, which
    // is why the key frame was always exact and only inter frames drifted.
    let deblock_tx_is_block_max = decision.is_inter && skip;
    if !writer.cdf_only && decision.tx_depth > 0 && !deblock_tx_is_block_max {
        let (txw, txh) = crate::leaf_funnel::txb_dims_at_depth(
            decision.width as usize,
            decision.height as usize,
            decision.tx_depth,
        );
        let cols = decision.width as usize / txw;
        let txbs = cols * (decision.height as usize / txh);
        for txb in 0..txbs {
            geom.record_tx_dims(
                block_x + (txb % cols) * txw,
                block_y + (txb / cols) * txh,
                txw,
                txh,
            );
        }
    }
}

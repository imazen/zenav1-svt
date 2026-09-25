use super::*;

impl EncodePipeline {
    #[inline(always)]
    pub(super) fn bd10_post_pass(
        bit_depth: u8,
        preset: i8,
        sharpness: i8,
        tune: u8,
        last_recon10_y: &mut Option<Vec<u16>>,
        last_recon10_uv: &mut Option<(Vec<u16>, Vec<u16>)>,
        chroma: Option<(&[u8], &[u8])>,
        hbd_source: &Option<HbdSource>,
        hbd_used: &mut bool,
        is_key: bool,
        sc_arm: crate::sc_detect::ScArm,
        md_lambda_base_update_type: Option<crate::port_rc_process::FrameUpdateType>,
        md_lambda_factor_update_type: crate::port_rc_process::FrameUpdateType,
        md_alt_lambda_factors: bool,
        lambda_mod_intra: i64,
        w: usize,
        h: usize,
        fmt: svtav1_types::chroma::ChromaFormat,
        acw: usize,
        ach: usize,
        sb_input: &[u8],
        in_stride: usize,
        sb_chroma_owned: &Option<(Vec<u8>, Vec<u8>)>,
        hbd_sb_owned: Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
        base_qindex: u8,
        picture_qp: u8,
        lw_bump: u32,
        coded_lossless: bool,
        primary_ref_cdfs: &Option<alloc::sync::Arc<crate::port_frame_cdf::FrameCdfs>>,
        c_quant: &Option<alloc::sync::Arc<crate::quant::CodingQuantCfg>>,
        sb_size: usize,
        sb_cols: usize,
        tile_grid: crate::entropy::obu::TileGrid,
        qindex_u: u8,
        qindex_v: u8,
        qm_levels: [u8; 3],
        seq_tools: crate::entropy::obu::SeqTools,
        inter_md_frame: Option<crate::inter_md_arm::InterMdFrame<'_>>,
        sb_inter_lambda: Option<Vec<crate::pd0::SbInterLambda>>,
        sb_enc_rdoq: Vec<bool>,
        all_trees: &mut [crate::partition::PartitionTree],
    ) -> Result<(), whereat::prelude::At<EncodeError>> {
        if bit_depth == 10 {
            // The native level pass accepts depth-zero transforms, including
            // filter-intra and directional prediction without edge filtering.
            // Use the actual sequence-header tool value. Monochrome disables
            // edge filtering; color's full-RD funnel handles lower presets.
            let bd10_edge_filter = seq_tools.enable_intra_edge_filter;
            // PARTIAL SB (2026-08-04): this used to be gated on
            // `w % 64 == 0 && h % 64 == 0` with the rationale that
            // "`tx_unit_hbd` is not partial-SB-aware". That named the wrong
            // function — `tx_unit_hbd` takes explicit `(w, h, stride, off)` and
            // is handed `rd: None` here, so it has no geometry term at all.
            // The real exposure was in the CALLERS, and all of it is now fixed:
            // `recon10` is SB-extent-sized (was ALIGNED-sized, so a straddling
            // write ran past the buffer or wrapped a row), the recon writes are
            // straddle-clipped like `commit_leaf`'s, the sources are the
            // SB-extent-padded `sb_input`/`sb_chroma_owned` twins, and the Split
            // arms walk quadrant SLOTS skipping off-frame origins instead of
            // zipping a fixed `(type, len)` offset table that a pruned child
            // list does not satisfy. See `bd10_reencode_luma` /
            // `bd10_reencode_node`.
            // HISTORICAL MEASUREMENT (task #94, before native luma context
            // wiring below): the bd10 FULL-RD funnel also
            // produces 10-bit coded levels, computed with each txb's REAL
            // entropy contexts — whereas this post-pass hardcodes the RDOQ
            // contexts to 0/0 (only correct where `real_coeff_ctx` is off).
            // Skipping the post-pass in favour of the funnel's levels was
            // therefore expected to be strictly better; it was A/B MEASURED on
            // the p6 bd10 grid and is NOT (4/20 byte-exact with the post-pass,
            // 3/20 without — `gradient 64x64 q12` regresses to a CDEF-strength
            // divergence). So the post-pass stays authoritative for the coded
            // levels until that is root-caused. The funnel's 10-bit levels are
            // still live where the post-pass does not reach: the neighbour
            // `cul` bytes that drive later blocks' coefficient contexts, and
            // the u8 chroma recon the CDEF/LR searches read.
            // The resulting gate remains: where the FULL-RD funnel ran, it
            // ALREADY produced this frame's
            // coded 10-bit levels and the committed 10-bit recon, computed
            // with each txb's REAL entropy contexts. This level-only post-pass
            // originally hardcoded RDOQ contexts to 0/0 — correct only where
            // `real_coeff_ctx` is off — so letting it run on top REPLACED
            // correct levels with ones quantized under the wrong contexts, and
            // the recon it writes then disagrees with the bitstream the funnel
            // decided. That is exactly the invariant `bd10_full_rd_supported`
            // documents ("the winner's 10-bit levels ARE the coded ones, so
            // the level-only re-encode post-pass is skipped"); it was
            // documented but never actually implemented in this gate.
            //
            // MEASURED (bd10, 128x128 gradient, presets 3 and 5, q12/q32/q55):
            // with both running, the port's 10-bit recon differs from C's by
            // 8194-11766 bytes and the tile payload diverges; with the
            // post-pass correctly skipped, the recon is byte-identical to C's
            // `svt_aom_get_recon_pic` dump. The eff-M9 band (preset >= 9) is
            // NOT full-RD, so the post-pass stays authoritative there — which
            // is why removing it wholesale regressed that band (the A/B noted
            // in docs/bd10-port-map.md) while removing it *conditionally* does
            // not.
            let bd10_full_rd = bd10_full_rd_supported(
                coded_lossless,
                bit_depth,
                preset,
                chroma.is_some(),
                is_key,
                w,
                h,
            );
            // `pcs->hbd_md` gates only the MD quantization depth
            // (`is_islice ? 2 : 0` at M6+, `is_base ? 2 : 0` at M0..M5 —
            // TRACED 2026-09-18: the johnny p6 cell's P-frame derives 0). The
            // residual 10-bit re-quantize this post-pass models is C's at TWO
            // different sites depending on `pic_bypass_encdec`:
            // - bypass off (bd10 video <= M7): the ENCODE pass
            //   (`av1_encode_loop` -> `svt_aom_quantize_inv_quantize` with
            //   `is_encode_pass = true`, `ed_ctx->bit_depth =
            //   encoder_bit_depth`).
            // - bypass on (bd10 video M8+, `get_bypass_encdec_default`,
            //   enc_mode_config.c:8426-8433): `encode_b` early-returns
            //   through `update_b` and ships the MD-committed levels — but
            //   `product_coding_loop.c:9649` first bumps `ctx->hbd_md = 2`
            //   for `bypass_encdec && encoder_bit_depth > 8 &&
            //   pd_pass == PD_PASS_1 && perform_md_recon`, so MDS3 itself
            //   quantizes the winner at TRUE 10-bit (`full_lambda_md[
            //   EB_10_BIT_MD]`). TRACED 2026-10-08 on vidyo4 p8: zero
            //   `is_encode_pass` quantize calls, and the MD-side `lam`
            //   equals the 10-bit chain exactly.
            // Either way the coded coefficients are a 10-bit re-quantize of
            // the committed TUs, so the post-pass runs in both modes.
            let bd10_postpass_runs = !bd10_full_rd
                && all_trees
                    .iter()
                    .all(|t| bd10_tree_supported(t, bd10_edge_filter, coded_lossless));
            // `update_skip_ctx_dc_sign_ctx` for THIS arm — C gates the
            // per-TU `get_txb_ctx` derivation (and its RDOQ input) on it
            // (`rate_est_ctrls` at enc_mode_config.c:6428). `for_preset`
            // bakes the allintra ladder (real ctx only <= M6); the video arm
            // is a flat `rate_est_level = 1` (:8942), so P/B-frames derive
            // REAL coefficient contexts at every preset — MEASURED on the
            // vidyo4 p8 cell, where C's MDS3 quantize read tsc=2 on a
            // split-TX TU whose coded left neighbour feeds the
            // `skip_contexts` table.
            let postpass_real_ctx =
                crate::rate_arm::rate_est_ctrls(crate::rate_arm::rate_est_level(
                    sc_arm,
                    crate::rate_arm::eff_enc_mode(sc_arm, preset),
                ))
                .1;
            // Diagnostic: which 10-bit canvas the post-filter searches (DLF
            // level, CDEF strength, Wiener LR) end up reading. The two
            // producers — the FULL-RD funnel's committed per-block recon and
            // this level-only post-pass — are gated differently, so "which one
            // is live" is the first question any recon-parity investigation
            // has to answer and it is not otherwise observable from outside.
            #[cfg(feature = "std")]
            if crate::dbgenv::bd10_postpass() {
                let unsupported = all_trees
                    .iter()
                    .filter(|t| !bd10_tree_supported(t, bd10_edge_filter, coded_lossless))
                    .count();
                eprintln!(
                    "BD10_POSTPASS runs={bd10_postpass_runs} \
                     unsupported_sbs={unsupported}/{} edge_filter={bd10_edge_filter}",
                    all_trees.len()
                );
            }
            if let Some(cq) = c_quant.as_ref().filter(|_| bd10_postpass_runs) {
                let shift = (bit_depth - 8) as u32;
                // Task #6 chunk 1: the REAL 10-bit source when the caller
                // entered through `try_encode_frame_*_hbd` (so the coded
                // levels carry the low 2 bits), else the `u8 << shift`
                // widening this site always did.
                // It is the SB-EXTENT-padded plane (`sb_input` / `hbd_sb_owned`
                // at `in_stride`), not the aligned one: a straddling leaf's
                // residual gather reads the full block width. Identical to the
                // aligned plane on every 64-aligned frame, where
                // `sb_input == encode_input` and `in_stride == w`.
                let src10: alloc::vec::Vec<u16> = match hbd_sb_owned
                    .as_ref()
                    .map(|(y, _, _)| y)
                    .or_else(|| hbd_source.as_ref().map(|h| &h.y))
                {
                    Some(y10) => {
                        debug_assert_eq!(y10.len(), sb_input.len());
                        *hbd_used = true;
                        y10.clone()
                    }
                    None => sb_input.iter().map(|&s| (s as u16) << shift).collect(),
                };
                // bd10 ENCODE-PASS lambda (C `pic_full_lambda[EB_10_BIT_MD]`,
                // assigned in `reset_enc_dec` via `svt_aom_lambda_assign(..,
                // EB_TEN_BIT, base_q_idx, multiply_lambda = true)`,
                // enc_dec_process.c:184-188). That is `compute_rd_mult` at
                // 10 bits — the update-type base multiplier + the
                // `rd_frame_type_factor[1]` row + scale — then `*= 16`. It
                // does NOT take `av1_lambda_assign_md`'s `lambda_weight` or
                // `lambda_mod_intra` (md_process.c:730-753): those are the MD
                // ladders, and this post-pass models EncDec.
                //
                // On a key frame `kf_full_lambda_bd10` computes the same
                // chain with the KF base multiplier (3.3) — equal to
                // `lambda_assign` here once `pcs->lambda_weight` is 0, which
                // is every measured key-frame cell; keep the proven call.
                let lambda_bd10 = u64::from(if is_key || coded_lossless {
                    crate::pd0::kf_full_lambda_bd10(base_qindex, picture_qp as u32, preset)
                } else {
                    // `ed_ctx->md_ctx->full_lambda_md[EB_10_BIT_MD]` — the
                    // `av1_lambda_assign_md` chain, NOT `pic_full_lambda`:
                    // coding_loop.c:436 hands the encode-pass quantizer the
                    // MD lambda, so `lambda_weight`/`lambda_mod_intra` DO
                    // apply (at q40 the weight is 150 → λ ×1.17).
                    crate::pd0::inter_full_lambda_bd10(
                        base_qindex,
                        md_lambda_base_update_type
                            .expect("an inter frame always has a picture decision"),
                        md_lambda_factor_update_type,
                        md_alt_lambda_factors,
                        0,
                        lambda_mod_intra,
                        crate::pd0::frame_lambda_weight_for_preset(
                            preset,
                            picture_qp as u32,
                            tune == crate::tune::TUNE_IQ,
                            lw_bump,
                        ),
                    )
                });
                // `ed_ctx->md_skip_blk` (coding_loop.c:387/464): C's encode
                // pass force-zeroes every TU — luma AND chroma — when MD
                // committed the block as skip. The chroma pass runs after the
                // luma walk overwrites the leaf eobs, so the funnel's
                // commitment is collected into this set on the way through.
                let mut committed_skip: alloc::collections::BTreeSet<(u32, u32)> =
                    alloc::collections::BTreeSet::new();
                let recon10 = bd10_reencode_luma(
                    all_trees,
                    sb_cols,
                    sb_size,
                    &tile_grid,
                    w,
                    h,
                    &src10,
                    in_stride,
                    base_qindex,
                    cq.rdoq_level,
                    lambda_bd10,
                    cq.allintra_rd_mult,
                    postpass_real_ctx,
                    bd10_edge_filter,
                    bit_depth,
                    qm_levels[0],
                    sharpness,
                    // The DPB's reference pictures, whose 10-bit twin the INTER
                    // arm predicts from. `None` on a key frame, where no leaf
                    // can be inter.
                    inter_md_frame.as_ref().map(|f| &f.padded_by_ref),
                    // The encode-pass RDOQ rate table is estimated from
                    // `pcs->md_frame_context`, seeded from the primary ref's
                    // saved CDFs (enc_dec_process.c:2817 +
                    // md_config_process.c:292) — same source as `fun_rates`.
                    primary_ref_cdfs.as_deref(),
                    &mut committed_skip,
                    // Per-SB `full_lambda_md[EB_10_BIT_MD]` — C's encode pass
                    // re-runs `av1_lambda_assign_md` for every superblock
                    // (mode_decision_configure_sb), so the RDOQ lambda varies
                    // by SB through `me_q_index - base_q_idx` even with
                    // `delta_q_present == 0`.
                    sb_inter_lambda.as_deref(),
                    // Per-SB `rdoq_ctrls->enabled` — the light-PD1 encode arm
                    // can turn RDOQ off where `pcs->rdoq_level` kept it
                    // (`sig_deriv_enc_dec_light_pd1_default`, lpd1 > L4 →
                    // level 0). Uniform `rdoq_level != 0` on key frames, so
                    // the stills gates are byte-inert.
                    Some(&sb_enc_rdoq),
                )?;
                // bd10 CHROMA re-encode (task #94): recompute chroma levels at
                // bd10 too — the luma pass above leaves chroma at the u8 MD
                // decision, which diverges on content whose subsampled chroma
                // carries a coded residual (e.g. `diag`). Gated identically
                // (complete-SB + bd10_tree_supported, which rejects CfL /
                // directional-uv-with-edge-filter). Flat-chroma content
                // (gradient/uniform) re-encodes to the same zero result, so bd8
                // and the existing bd10 gate cells stay byte-unchanged. Chroma
                // qindex == base_qindex in mainline (all FH chroma deltas 0),
                // matching the walk's `base_q_idx` chroma coding.
                if let Some((u_src, v_src)) = sb_chroma_owned.as_ref() {
                    // Task #6 chunk 1: real 10-bit chroma when supplied. Both
                    // sides are the SB-extent shape (`sb_chroma_owned` /
                    // `hbd_sb_owned`), which is the untouched aligned chroma on
                    // a 64-aligned frame — so the two planes match
                    // element-for-element either way.
                    let hbd_uv = hbd_sb_owned
                        .as_ref()
                        .map(|(_, u, v)| (u, v))
                        .or_else(|| hbd_source.as_ref().map(|h| (&h.u, &h.v)))
                        .filter(|(u, _)| !u.is_empty());
                    let (u10, v10): (alloc::vec::Vec<u16>, alloc::vec::Vec<u16>) = match hbd_uv {
                        Some((hu, hv)) => {
                            debug_assert_eq!(hu.len(), u_src.len());
                            *hbd_used = true;
                            (hu.clone(), hv.clone())
                        }
                        None => (
                            u_src.iter().map(|&s| (s as u16) << shift).collect(),
                            v_src.iter().map(|&s| (s as u16) << shift).collect(),
                        ),
                    };
                    // The pass's residual gather reads the full TX width at
                    // `cstride`, so a right-straddle TU on an `acw`-strided
                    // plane WRAPS into the next row's real samples. C reads
                    // its chroma picture at a border-inclusive stride whose
                    // pad holds the replicated right edge
                    // (`svt_aom_generate_padding16_bit`, resize.c:1064 —
                    // `pad_input_pictures` on the non-resize path). Give the
                    // source the same SB-extent-stride, edge-replicated shape
                    // the funnel builds at `padded_chroma10`
                    // (~pipeline.rs:14106). MEASURED: uniform 128 q20 d10 at
                    // p9/p10/p13 — C coded a chroma txb the next-row read
                    // quantized to eob 0 (27B vs C's 28B OBU, ±1 chroma LSB
                    // in the right-region recon).
                    let cstride = fmt.chroma_width(w.div_ceil(sb_size) * sb_size);
                    let (u10, v10) = if cstride != acw {
                        let md_ch = fmt.chroma_height(h.div_ceil(sb_size) * sb_size);
                        (
                            pad_plane_replicate_u16(&u10, acw, acw, ach, cstride, md_ch)?,
                            pad_plane_replicate_u16(&v10, acw, acw, ach, cstride, md_ch)?,
                        )
                    } else {
                        (u10, v10)
                    };
                    let uv10 = bd10_reencode_chroma(
                        all_trees,
                        sb_cols,
                        sb_size,
                        &tile_grid,
                        w,
                        h,
                        &u10,
                        &v10,
                        cstride,
                        // The 10-bit LUMA recon the pass above just produced —
                        // the CfL AC source for UV_CFL_PRED leaves. C reads the
                        // same thing (`cfl_temp_luma_recon16bit`), and it is
                        // fully committed here because the luma re-encode walks
                        // the entire frame before chroma starts.
                        &recon10,
                        w,
                        // base_qindex sources the frame-level coeff-rate context
                        // (`cfc`); qindex_u/qindex_v drive the per-plane chroma
                        // quant tables (== base in mainline). See the fn doc.
                        base_qindex,
                        qindex_u,
                        qindex_v,
                        cq.rdoq_level,
                        lambda_bd10,
                        cq.allintra_rd_mult,
                        postpass_real_ctx,
                        bd10_edge_filter,
                        bit_depth,
                        [qm_levels[1], qm_levels[2]],
                        sharpness,
                        inter_md_frame.as_ref().map(|f| &f.padded_by_ref),
                        primary_ref_cdfs.as_deref(),
                        &committed_skip,
                        sb_inter_lambda.as_deref(),
                        Some(&sb_enc_rdoq),
                    )?;
                    // Crop the SB-extent canvases to the in-frame planes every
                    // downstream consumer expects (the bd10 deblock-level /
                    // CDEF-strength / Wiener-LR searches compare them against
                    // `w*h` and `(w>>ss_x)*(h>>ss_y)` sources at the ALIGNED stride).
                    // The canvases are `cstride`-strided (== `acw` on a
                    // 64-aligned frame, where the crop degenerates to a prefix).
                    let mut cu = svtav1_types::try_vec![0u16; acw * ach]?;
                    let mut cv = svtav1_types::try_vec![0u16; acw * ach]?;
                    for r in 0..ach {
                        cu[r * acw..(r + 1) * acw]
                            .copy_from_slice(&uv10.0[r * cstride..r * cstride + acw]);
                        cv[r * acw..(r + 1) * acw]
                            .copy_from_slice(&uv10.1[r * cstride..r * cstride + acw]);
                    }
                    *last_recon10_uv = Some((cu, cv));
                }
                *last_recon10_y = Some(recon10[..w * h].to_vec());
            }
            // At hbd_md == 0 (non-I-slice, enc_mode > M5 — the `is_key`/`is_base`
            // terms C encodes in `pcs->hbd_md`, TRACED 2026-09-18) C's mode
            // decision runs entirely on the MSB-truncated u8 picture — the u16
            // source's consumption IS that truncation
            // (`svt_convert_8bit_to_16bit` is a plain copy, pack_unpack_c.c:198;
            // the 16-bit pipeline then carries 0..255 values). Mark it consumed
            // so the no-silent-truncation guard does not fire on exactly the
            // frames where truncation is the C-faithful behavior.
            if hbd_source.is_some() && !is_key && !bd10_full_rd {
                *hbd_used = true;
            }
        }
        Ok(())
    }
}

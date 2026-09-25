use super::*;

#[inline(always)]
pub(super) fn derive_stale_vars(
    stats_src: Option<(Vec<u8>, usize, usize)>,
) -> Option<Vec<crate::pd0::SbVariance>> {
    // Superres chunk B.4: C's per-b64 variance array (`pcs->variance`) is
    // built by picture analysis on the FULL-RESOLUTION picture, and
    // `scale_pcs_params` (resize.c:1434) re-inits the b64/SB geometry for
    // the coded size WITHOUT recomputing it — so every PD0 / dc-only gate
    // downstream reads full-res variances through the SMALLER coded-grid
    // indices. Reproduce that exactly: build the array over the full-res
    // grid in raster order here, and index it with the coded grid's linear
    // SB index at the search. `None` on every non-superres path -> the
    // variance is recomputed from the coded source, unchanged.
    let stale_vars: Option<alloc::vec::Vec<crate::pd0::SbVariance>> =
        stats_src.as_ref().map(|(orig, ow, oh)| {
            let (ext_w, ext_h) = (ow.div_ceil(64) * 64, oh.div_ceil(64) * 64);
            let mut padded = alloc::vec![0u8; ext_w * ext_h];
            for r in 0..*oh {
                padded[r * ext_w..r * ext_w + ow].copy_from_slice(&orig[r * ow..(r + 1) * ow]);
            }
            crate::frame_geom::pad_input_plane(
                &mut padded,
                &crate::frame_geom::FrameDims::new(*ow, *oh),
                64,
            );
            let (cols, rows) = (ext_w / 64, ext_h / 64);
            let mut v = alloc::vec::Vec::with_capacity(cols * rows);
            for by in 0..rows {
                for bx in 0..cols {
                    v.push(crate::pd0::compute_b64_variance(
                        &padded,
                        ext_w,
                        bx * 64,
                        by * 64,
                    ));
                }
            }
            v
        });
    stale_vars
}

#[inline(always)]
pub(super) fn build_sb_chroma(
    chroma: Option<(&[u8], &[u8])>,
    fmt: svtav1_types::chroma::ChromaFormat,
    acw: usize,
    ach: usize,
    ext_w: usize,
    ext_h: usize,
) -> Result<Option<(Vec<u8>, Vec<u8>)>, whereat::prelude::At<EncodeError>> {
    let sb_chroma_owned: Option<(alloc::vec::Vec<u8>, alloc::vec::Vec<u8>)> = chroma
        .map(
            |(u, v)| -> crate::EncodeResult<(alloc::vec::Vec<u8>, alloc::vec::Vec<u8>)> {
                let (ext_cw, ext_ch_h) = (fmt.chroma_width(ext_w), fmt.chroma_height(ext_h));
                Ok(if ext_ch_h == ach && ext_cw == acw {
                    // Full-SB (or 64-aligned) frame: exact aligned chroma,
                    // byte-identical to the pre-#95 source.
                    (u.to_vec(), v.to_vec())
                } else {
                    // Partial SB: `acw`-strided rows, edge-replicating the last
                    // real chroma row. Enough rows to cover BOTH a height-
                    // straddle read (reaches `ext_ch_h`) AND a right-straddle
                    // read that wraps down into later stride rows. For gradient
                    // (uniform chroma) every padded byte equals the true edge,
                    // so the reads match C's SB-extent pad; other content is
                    // decodable (the boundary chroma differs from C's crop).
                    let n_rows = ext_ch_h + ext_cw.div_ceil(acw) + 2;
                    let cap = n_rows * acw;
                    let mut up = svtav1_types::try_vec![0u8; cap]?;
                    let mut vp = svtav1_types::try_vec![0u8; cap]?;
                    for r in 0..n_rows {
                        let sr = r.min(ach - 1);
                        up[r * acw..(r + 1) * acw].copy_from_slice(&u[sr * acw..(sr + 1) * acw]);
                        vp[r * acw..(r + 1) * acw].copy_from_slice(&v[sr * acw..(sr + 1) * acw]);
                    }
                    (up, vp)
                })
            },
        )
        .transpose()?;
    Ok(sb_chroma_owned)
}

#[inline(always)]
pub(super) fn build_hbd_sb(
    hbd_source: &Option<HbdSource>,
    w: usize,
    h: usize,
    fmt: svtav1_types::chroma::ChromaFormat,
    acw: usize,
    ach: usize,
    ext_w: usize,
    ext_h: usize,
    sb_input_owned: &Option<Vec<u8>>,
) -> Result<Option<(Vec<u16>, Vec<u16>, Vec<u16>)>, whereat::prelude::At<EncodeError>> {
    let hbd_sb_owned: Option<(
        alloc::vec::Vec<u16>,
        alloc::vec::Vec<u16>,
        alloc::vec::Vec<u16>,
    )> = match hbd_source.as_ref() {
        Some(hbd) if sb_input_owned.is_some() => {
            let y = pad_plane_replicate_u16(&hbd.y, w, w, h, ext_w, ext_h)?;
            let (u, v) = if hbd.u.is_empty() {
                (alloc::vec::Vec::new(), alloc::vec::Vec::new())
            } else {
                let (ext_cw, ext_ch_h) = (fmt.chroma_width(ext_w), fmt.chroma_height(ext_h));
                let n_rows = ext_ch_h + ext_cw.div_ceil(acw) + 2;
                let mut up = svtav1_types::try_vec![0u16; n_rows * acw]?;
                let mut vp = svtav1_types::try_vec![0u16; n_rows * acw]?;
                for r in 0..n_rows {
                    let sr = r.min(ach - 1);
                    up[r * acw..(r + 1) * acw].copy_from_slice(&hbd.u[sr * acw..(sr + 1) * acw]);
                    vp[r * acw..(r + 1) * acw].copy_from_slice(&hbd.v[sr * acw..(sr + 1) * acw]);
                }
                (up, vp)
            };
            Some((y, u, v))
        }
        _ => None,
    };
    Ok(hbd_sb_owned)
}

#[inline(always)]
pub(super) fn fill_canvas10(
    w: usize,
    h: usize,
    ss_x: usize,
    ss_y: usize,
    acw: usize,
    sb_size: usize,
    tile_grid: crate::entropy::obu::TileGrid,
    tile_recons: &[(
        Vec<u8>,
        Vec<crate::partition::PartitionTree>,
        Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
        Vec<bool>,
    )],
    canvas10: &mut Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
) {
    if let Some((cy, cu, cv)) = canvas10.as_mut() {
        for (tile_idx, t) in tile_recons.iter().enumerate() {
            let Some((ty, tu, tv)) = t.2.as_ref() else {
                continue;
            };
            let (r0, r1) = tile_grid.row_span(tile_idx / tile_grid.tile_cols);
            let (c0, c1) = tile_grid.col_span(tile_idx % tile_grid.tile_cols);
            let (y0, y1) = (r0 * sb_size, (r1 * sb_size).min(h));
            let (x0, x1) = (c0 * sb_size, (c1 * sb_size).min(w));
            for r in y0..y1 {
                cy[r * w + x0..r * w + x1].copy_from_slice(&ty[r * w + x0..r * w + x1]);
            }
            let (cw, cxs, cxe) = (acw, x0 >> ss_x, x1 >> ss_x);
            let cst = cw;
            for r in (y0 >> ss_y)..(y1 >> ss_y) {
                cu[r * cw + cxs..r * cw + cxe].copy_from_slice(&tu[r * cst + cxs..r * cst + cxe]);
                cv[r * cw + cxs..r * cw + cxe].copy_from_slice(&tv[r * cst + cxs..r * cst + cxe]);
            }
        }
    }
}

#[inline(always)]
pub(super) fn derive_dlf_level(
    is_single_frame: bool,
    dlf_enc_mode: i8,
    dlf_resolution: crate::port_enc_mode_config::ResolutionRange,
    dlf_is_base: bool,
    dlf_is_not_last_layer: u8,
    dlf_ref_skip_percentage: u8,
) -> u8 {
    let dlf_level = if is_single_frame {
        // `get_dlf_level_allintra(dlf_enc_mode, fast_decode, resolution)`.
        crate::port_enc_mode_config::leaf::get_dlf_level_allintra(
            dlf_enc_mode,
            DLF_FAST_DECODE,
            dlf_resolution,
        )
    } else {
        // `get_dlf_level_default(pcs, dlf_enc_mode, is_not_last_layer,
        //  fast_decode, resolution, is_base)`.
        //
        // `coeff_lvl` is read only in the M10..M11 arm, and there both
        // branches yield 6 when `is_base` — which every KEY frame is
        // (`temporal_layer_index == 0`) — so the value passed cannot
        // change a key frame's level. `ref_skip_percentage` feeds
        // `dlf_level_modulation`, which C runs only when `!is_base`;
        // modulation mode 3 can zero an otherwise-enabled level when the
        // references are >95% skip (MEASURED: hier-2 LD-CBR poc6, a TL1
        // frame whose level-6 became 0 on refs at 100% skip).
        crate::port_enc_mode_config::leaf::get_dlf_level_default(
            dlf_enc_mode,
            dlf_is_not_last_layer,
            DLF_FAST_DECODE,
            dlf_resolution,
            dlf_is_base,
            crate::port_enc_mode_config::InputCoeffLvl::Normal,
            dlf_ref_skip_percentage,
        )
    };
    dlf_level
}

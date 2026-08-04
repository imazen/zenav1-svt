//! Differential parity for the segmentation vertical (`segmentation.c` +
//! the segment-id entropy helpers in `entropy_coding.c`).
//!
//! # Evidence tiers
//!
//! **Tier 1 (real exported C symbol, driven through `svtav1-cref`)** —
//! everything in this file:
//! * `svt_av1_neg_interleave`             (entropy_coding.c:4825)
//! * `svt_av1_get_spatial_seg_prediction` (entropy_coding.c:4777), which
//!   transitively pins the `static INLINE` `svt_aom_get_segment_id` (:4727)
//! * `svt_av1_update_segmentation_map`    (entropy_coding.c:4847)
//! * `calculate_segmentation_data`        (segmentation.c:249)
//! * `svt_aom_setup_segmentation`         (segmentation.c:228), which
//!   transitively pins `find_segment_qps` (segmentation.c:262)
//! * `svt_aom_apply_segmentation_based_quantization` (segmentation.c:136),
//!   which transitively pins the `static` `get_variance_for_cu` (:23)
//! * `write_segment_id`                   (entropy_coding.c:4867) — the
//!   coded BYTES from the real `od_ec` coder, plus the adapted
//!   `spatial_pred_seg_cdf` and the stamped map
//! * `svt_aom_wb_write_inv_signed_literal` (entropy_coding.c:1377)
//! * `svt_aom_segmentation_feature_{bits,signed,max}` (segmentation_params.c)
//! * `FRAME_CONTEXT.seg.{tree,pred,spatial_pred_seg}_cdf`
//!   (cabac_context_model.c:652-664)
//!
//! **NOT covered here (no exported symbol, and no exported caller reaches
//! them) — these fall back to the project's WEAKER tier, hand-derived
//! vectors traced against the C source, in the ported modules' own `#[cfg
//! (test)]` blocks:**
//! * `roi_map_setup_segmentation` / `roi_map_apply_segmentation_based_
//!   quantization` (`static`, segmentation.c:160 and :87). Reachable only
//!   via `ppcs->roi_map_evt != NULL`; a shim could supply the ROI map, but
//!   the setup arm then calls `svt_av1_pick_filter_level_by_q`, which reads
//!   `ppcs->input_resolution`, `tot_ref_frame_types`, `ref_pic_ptr_array`,
//!   `frm_hdr` and `frame_is_boosted(ppcs)` — more live encoder state than a
//!   calloc'd struct can honestly stand in for.
//! * `encode_segmentation`'s LOOP STRUCTURE (`static`,
//!   entropy_coding.c:2247). Its payload primitive is pinned at tier 1
//!   above; the enable-bit / feature-order loop around it is asserted
//!   against a hand-decoded bit layout in
//!   `segmentation_params_header_from_c_derived_state`.
//! * `write_inter_segment_id` (`static`, entropy_coding.c:4889) — its
//!   pre/post-skip routing is asserted structurally in
//!   `svtav1_entropy::context`'s test module; its only real work is the
//!   tier-1-pinned `write_segment_id` call.

use svtav1_cref as cref;
use svtav1_encoder::segmentation as seg;
use svtav1_entropy::context::{
    FrameContext, SEG_TEMPORAL_PRED_CTXS, SPATIAL_PREDICTION_PROBS, SegmentationMap,
    get_spatial_seg_prediction, neg_interleave,
};
use svtav1_types::block::BlockSize;
use svtav1_types::restoration::MAX_SEGMENTS;
use svtav1_types::segmentation::{
    SEG_LVL_MAX, SEGMENTATION_FEATURE_BITS, SEGMENTATION_FEATURE_MAX, SEGMENTATION_FEATURE_SIGNED,
    SegmentationParams,
};

/// Deterministic 32-bit LCG (Numerical Recipes constants) so every sweep is
/// reproducible without pulling in a dependency.
struct Lcg(u32);
impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.0
    }
    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

/// Serializes every test that calls `cref::fc_init` — it initializes a
/// PROCESS-GLOBAL C frame context (same hazard the entropy crate's
/// `c_parity.rs` documents).
fn fc_guard() -> std::sync::MutexGuard<'static, ()> {
    static FC_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    FC_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// Feature tables (exported const data)
// ---------------------------------------------------------------------------

#[test]
fn feature_tables_match_c() {
    let (bits, sgn, maxv) = cref::segmentation_feature_tables();
    assert_eq!(bits, SEGMENTATION_FEATURE_BITS, "feature_bits diverges");
    assert_eq!(sgn, SEGMENTATION_FEATURE_SIGNED, "feature_signed diverges");
    assert_eq!(maxv, SEGMENTATION_FEATURE_MAX, "feature_max diverges");
}

// ---------------------------------------------------------------------------
// FRAME_CONTEXT.seg CDF defaults
// ---------------------------------------------------------------------------

#[test]
fn seg_cdf_defaults_match_c() {
    let _g = fc_guard();
    // The seg tables are q-INDEPENDENT (svt_aom_init_mode_probs), so any
    // qindex works; 60 matches the sibling mode-table drift test.
    cref::fc_init(60);
    let fc = FrameContext::new_default();
    assert_eq!(
        fc.seg_tree_cdf.to_vec(),
        cref::fc_table(cref::FcTable::SegTree),
        "seg.tree_cdf"
    );
    let pred: Vec<u16> = fc.seg_pred_cdf.iter().flatten().copied().collect();
    assert_eq!(pred, cref::fc_table(cref::FcTable::SegPred), "seg.pred_cdf");
    let spatial: Vec<u16> = fc.spatial_pred_seg_cdf.iter().flatten().copied().collect();
    assert_eq!(
        spatial,
        cref::fc_table(cref::FcTable::SegSpatialPred),
        "seg.spatial_pred_seg_cdf"
    );
    // Shape sanity: the C arrays are CDF_SIZE(MAX_SEGMENTS)=9 and CDF_SIZE(2)=3.
    assert_eq!(fc.seg_pred_cdf.len(), SEG_TEMPORAL_PRED_CTXS);
    assert_eq!(fc.spatial_pred_seg_cdf.len(), SPATIAL_PREDICTION_PROBS);
}

// ---------------------------------------------------------------------------
// svt_av1_neg_interleave — EXHAUSTIVE over the whole legal domain
// ---------------------------------------------------------------------------

#[test]
fn neg_interleave_matches_c_exhaustively() {
    // C asserts `x < max`. `max` is `last_active_seg_id + 1` (1..=8) and the
    // spatial prediction `ref` is a segment id (0..=7), so this sweep is the
    // COMPLETE reachable domain, not a sample.
    let mut cases = 0usize;
    for max in 1..=MAX_SEGMENTS as i32 {
        for reference in 0..MAX_SEGMENTS as i32 {
            for x in 0..max {
                let c = cref::neg_interleave(x, reference, max);
                let r = neg_interleave(x, reference, max);
                assert_eq!(c, r, "neg_interleave(x={x}, ref={reference}, max={max})");
                cases += 1;
            }
        }
    }
    // 8 refs x sum(max=1..8) = 8 * 36
    assert_eq!(
        cases, 288,
        "domain shrank — the sweep is no longer exhaustive"
    );
}

// ---------------------------------------------------------------------------
// calculate_segmentation_data
// ---------------------------------------------------------------------------

#[test]
fn calculate_segmentation_data_matches_c() {
    let mut rng = Lcg(0x1234_5678);
    for iter in 0..2000 {
        let mut enabled = [[0i16; SEG_LVL_MAX]; MAX_SEGMENTS];
        for row in enabled.iter_mut() {
            for cell in row.iter_mut() {
                // Sparse: mostly zero, so `last_active_seg_id` lands all over
                // the range instead of pinning at 7 every time.
                *cell = if rng.below(5) == 0 { 1 } else { 0 };
            }
        }
        // Exercise the accumulate-don't-clear behaviour with nonzero seeds.
        let seed_last = rng.below(8) as u8;
        let seed_pre = rng.below(2) as u8;

        let (c_last, c_pre) = cref::calculate_segmentation_data(&enabled, seed_last, seed_pre);

        let mut sp = SegmentationParams {
            feature_enabled: enabled,
            last_active_seg_id: seed_last,
            seg_id_pre_skip: seed_pre,
            ..SegmentationParams::default()
        };
        seg::calculate_segmentation_data(&mut sp);
        assert_eq!(
            (c_last, c_pre),
            (sp.last_active_seg_id, sp.seg_id_pre_skip),
            "calculate_segmentation_data iter {iter}, enabled={enabled:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// svt_aom_setup_segmentation (pins find_segment_qps end to end)
// ---------------------------------------------------------------------------

/// Build one `b64_total_count`-row variance plane in the shape C allocates
/// (`EB_MALLOC_2D(variance, b64_total_count, 85)`, pcs.c:1280).
fn make_variance(
    b64_total_count: u32,
    rng: &mut Lcg,
    spread: u32,
) -> (Vec<u16>, Vec<[u16; seg::VARIANCE_BLOCK_COUNT]>) {
    let mut flat = Vec::with_capacity((b64_total_count as usize) * seg::VARIANCE_BLOCK_COUNT);
    let mut rows = Vec::with_capacity(b64_total_count as usize);
    for _ in 0..b64_total_count {
        let mut row = [0u16; seg::VARIANCE_BLOCK_COUNT];
        for cell in row.iter_mut() {
            *cell = rng.below(spread) as u16;
        }
        flat.extend_from_slice(&row);
        rows.push(row);
    }
    (flat, rows)
}

#[test]
fn setup_segmentation_matches_c() {
    let mut rng = Lcg(0xC0FF_EE01);
    // Sweep the variance dynamic range hard: `find_segment_qps` is entirely
    // driven by log2(min)/log2(max)/log2(avg), and the interesting corners are
    // the int16 bin-edge truncation (high spread) and the all-zero frame
    // (log2f_safe's `| 1` guard).
    for &spread in &[1u32, 2, 17, 256, 4096, 40000, 65535] {
        for &b64_total_count in &[1u32, 3, 16, 63] {
            let (flat, rows) = make_variance(b64_total_count, &mut rng, spread);
            let c = cref::setup_segmentation(
                1,
                &flat,
                b64_total_count,
                seg::VARIANCE_BLOCK_COUNT as u32,
            );
            let mut r = SegmentationParams::default();
            seg::setup_segmentation(&mut r, 1, &rows, b64_total_count);

            let ctx = format!("spread={spread} b64_total_count={b64_total_count}");
            assert_eq!(c.enabled, r.segmentation_enabled, "enabled ({ctx})");
            assert_eq!(
                c.update_map, r.segmentation_update_map,
                "update_map ({ctx})"
            );
            assert_eq!(
                c.temporal_update, r.segmentation_temporal_update,
                "temporal_update ({ctx})"
            );
            assert_eq!(
                c.update_data, r.segmentation_update_data,
                "update_data ({ctx})"
            );
            assert_eq!(
                c.variance_bin_edge, r.variance_bin_edge,
                "variance_bin_edge ({ctx})"
            );
            assert_eq!(c.feature_data, r.feature_data, "feature_data ({ctx})");
            assert_eq!(
                c.feature_enabled, r.feature_enabled,
                "feature_enabled ({ctx})"
            );
            assert_eq!(
                c.last_active_seg_id, r.last_active_seg_id,
                "last_active_seg_id ({ctx})"
            );
            assert_eq!(
                c.seg_id_pre_skip, r.seg_id_pre_skip,
                "seg_id_pre_skip ({ctx})"
            );
        }
    }
}

#[test]
fn setup_segmentation_aq_mode_gate_matches_c() {
    let mut rng = Lcg(0xAB_CDEF01);
    let (flat, rows) = make_variance(4, &mut rng, 1000);
    for aq_mode in 0u8..=4 {
        let c = cref::setup_segmentation(aq_mode, &flat, 4, seg::VARIANCE_BLOCK_COUNT as u32);
        let mut r = SegmentationParams::default();
        seg::setup_segmentation(&mut r, aq_mode, &rows, 4);
        assert_eq!(c.enabled, r.segmentation_enabled, "aq_mode {aq_mode}");
        assert_eq!(c.feature_data, r.feature_data, "aq_mode {aq_mode}");
        assert_eq!(c.feature_enabled, r.feature_enabled, "aq_mode {aq_mode}");
        assert_eq!(
            c.variance_bin_edge, r.variance_bin_edge,
            "aq_mode {aq_mode}"
        );
    }
}

// ---------------------------------------------------------------------------
// svt_aom_apply_segmentation_based_quantization (pins get_variance_for_cu)
// ---------------------------------------------------------------------------

/// Sweeps every `BlockSize` at every legal origin inside a 64x64 SB, at five
/// base qindexes and three variance regimes.
///
/// `b64_total_count` is 4 and the probed SB is index 0, ON PURPOSE: C's
/// `BLOCK_16X8` arm computes `index1 = index0 + org_y` with `org_y` in
/// PIXELS, which for `org_y >= 16` runs past the probed b64's 85-entry row
/// into the NEXT b64's samples (the rows are slices of ONE `EB_MALLOC_2D`
/// allocation). Probing a non-last b64 keeps that read inside the allocation
/// so C has a defined value and the differential is meaningful; probing the
/// LAST b64 would be a heap over-read in C (see `get_variance_for_cu`'s
/// PORT-NOTE) and is deliberately not swept.
#[test]
fn apply_segmentation_based_quantization_matches_c() {
    let mut rng = Lcg(0x0BAD_F00D);
    const NB64: u32 = 4;
    const BC: usize = seg::VARIANCE_BLOCK_COUNT;
    // Every BlockSize the C switch names, plus the 128 sizes that fall into
    // its `default:` arm.
    let sizes = BlockSize::ALL;
    let mut checked = 0usize;
    for &spread in &[4u32, 512, 60000] {
        // A realistic parameter set: run the real setup first so the bin
        // edges and offsets are exactly what C would have produced.
        let (flat, rows) = make_variance(NB64, &mut rng, spread);
        let params = cref::setup_segmentation(1, &flat, NB64, BC as u32);
        let mut r_params = SegmentationParams::default();
        seg::setup_segmentation(&mut r_params, 1, &rows, NB64);
        assert_eq!(params.variance_bin_edge, r_params.variance_bin_edge);
        // The port's `variance` argument is the contiguous plane FROM the
        // probed b64's row onward — exactly what C's `variance[sb_index]`
        // points at inside the single EB_MALLOC_2D allocation.
        let sb_index = 0u32;
        let plane_from_row = &flat[(sb_index as usize) * BC..];

        for &base_q_idx in &[1i32, 5, 40, 120, 255] {
            for &bsize in sizes.iter() {
                // C's index math assumes the origin is a multiple of the
                // block size within a 64x64 SB, so sweep exactly those.
                let bw = block_wide(bsize);
                let bh = block_high(bsize);
                let mut org_y = 0;
                while org_y < 64 {
                    let mut org_x = 0;
                    while org_x < 64 {
                        let c = cref::apply_segmentation_based_quantization(
                            &params.variance_bin_edge,
                            &params.feature_data,
                            base_q_idx,
                            &flat,
                            NB64,
                            BC as u32,
                            sb_index,
                            bsize as i32,
                            org_x,
                            org_y,
                        );
                        let r = seg::apply_segmentation_based_quantization(
                            &r_params,
                            plane_from_row,
                            bsize,
                            org_x,
                            org_y,
                            base_q_idx,
                        );
                        assert_eq!(
                            c, r,
                            "apply_seg_quant spread={spread} q={base_q_idx} \
                             bsize={bsize:?} org=({org_x},{org_y})"
                        );
                        checked += 1;
                        org_x += bw.max(8);
                    }
                    org_y += bh.max(8);
                }
            }
        }
    }
    assert!(checked > 4000, "sweep shrank: only {checked} cells");
}

/// Pins the exact C over-read documented on `get_variance_for_cu`: a
/// `BLOCK_16X8` leaf at `org_y = 16` reads `variance_ptr[index0 + 16]`,
/// which is 16 entries past `index0` and therefore past the 85-entry row
/// once `index0 >= 69`. Nothing about that is hypothetical — this asserts
/// the port reads the SAME cross-row sample the C build does.
#[test]
fn block_16x8_cross_row_overread_matches_c() {
    const BC: usize = seg::VARIANCE_BLOCK_COUNT;
    const NB64: u32 = 2;
    // Row 0 all 1s; row 1 tagged with its own index times 2 so the exact
    // cross-row entry the C index math lands on is identifiable (a fixture
    // where row 1 were uniform could not tell `+org_y` from `+1`).
    let mut flat = vec![1u16; BC * NB64 as usize];
    for (j, v) in flat[BC..].iter_mut().enumerate() {
        *v = (2 * j + 2) as u16;
    }
    let params = cref::setup_segmentation(1, &flat, NB64, BC as u32);
    let mut r_params = SegmentationParams::default();
    let rows: Vec<[u16; BC]> = (0..NB64 as usize)
        .map(|i| {
            let mut r = [0u16; BC];
            r.copy_from_slice(&flat[i * BC..(i + 1) * BC]);
            r
        })
        .collect();
    seg::setup_segmentation(&mut r_params, 1, &rows, NB64);

    // org_x = 56, org_y = 56: index0 = 21 + 7 + 56 = 84 (last cell of row 0),
    // index1 = 84 + 56 = 140 = row 1's entry 55, which this fixture tags
    // 2*55 + 2 = 112. SVT_VAR_AVG2(1, 112) = 56 — a value only reachable by
    // reading THAT cell (the naive `index0 + 1` would land on row 1 entry 0
    // and give SVT_VAR_AVG2(1, 2) = 1).
    let c = cref::apply_segmentation_based_quantization(
        &params.variance_bin_edge,
        &params.feature_data,
        40,
        &flat,
        NB64,
        BC as u32,
        0,
        BlockSize::Block16x8 as i32,
        56,
        56,
    );
    let r = seg::apply_segmentation_based_quantization(
        &r_params,
        &flat,
        BlockSize::Block16x8,
        56,
        56,
        40,
    );
    assert_eq!(c, r, "BLOCK_16X8 cross-row read diverges");
    // And the raw kernel value is the cross-row average, not a row-0 value.
    // SVT_VAR_AVG2(row0[84]=1, row1[55]=112) = (1 + 112) >> 1 = 56.
    assert_eq!(
        seg::get_variance_for_cu(BlockSize::Block16x8, 56, 56, &flat),
        56,
        "BLOCK_16X8 must read row 1 entry 55 (index0 + org_y), not index0 + 1"
    );
}

/// `block_size_wide[bsize]` (common_utils.c:286), clamped to the 64x64 SB the
/// C index math assumes.
fn block_wide(b: BlockSize) -> i32 {
    const W: [i32; 22] = [
        4, 4, 8, 8, 8, 16, 16, 16, 32, 32, 32, 64, 64, 64, 128, 128, 4, 16, 8, 32, 16, 64,
    ];
    W[b as usize]
}
/// `block_size_high[bsize]` (common_utils.c:289).
fn block_high(b: BlockSize) -> i32 {
    const H: [i32; 22] = [
        4, 8, 4, 8, 16, 8, 16, 32, 16, 32, 64, 32, 64, 128, 64, 128, 16, 4, 32, 8, 64, 16,
    ];
    H[b as usize]
}

// ---------------------------------------------------------------------------
// svt_av1_get_spatial_seg_prediction (pins svt_aom_get_segment_id)
// ---------------------------------------------------------------------------

#[test]
fn get_spatial_seg_prediction_matches_c() {
    let mut rng = Lcg(0x5EED_1234);
    for &(mi_cols, mi_rows) in &[(4i32, 4i32), (16, 9), (32, 32)] {
        for trial in 0..40 {
            let map: Vec<u8> = (0..(mi_cols * mi_rows))
                .map(|_| rng.below(MAX_SEGMENTS as u32) as u8)
                .collect();
            let mut r_map = SegmentationMap::new(mi_cols as usize, mi_rows as usize);
            r_map.data.copy_from_slice(&map);

            for mi_row in 0..mi_rows {
                for mi_col in 0..mi_cols {
                    // C derives availability from the tile/frame edges; drive
                    // BOTH the real edge values and (for interior positions)
                    // the forced-false combinations, since a tile boundary
                    // makes an interior block report unavailable too.
                    let real = [(mi_col > 0, mi_row > 0)];
                    let forced: &[(bool, bool)] = if mi_col > 0 && mi_row > 0 {
                        &[(false, false), (true, false), (false, true)]
                    } else {
                        &[]
                    };
                    for &(left, up) in real.iter().chain(forced.iter()) {
                        let (c_pred, c_cdf) = cref::get_spatial_seg_prediction(
                            &map, mi_cols, mi_rows, mi_row, mi_col, left, up,
                        );
                        let (r_pred, r_cdf) = get_spatial_seg_prediction(
                            &r_map,
                            mi_row as usize,
                            mi_col as usize,
                            left,
                            up,
                        );
                        assert_eq!(
                            (c_pred as usize, c_cdf as usize),
                            (r_pred, r_cdf),
                            "spatial_seg_pred trial={trial} grid={mi_cols}x{mi_rows} \
                             mi=({mi_row},{mi_col}) left={left} up={up}"
                        );
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// svt_av1_update_segmentation_map
// ---------------------------------------------------------------------------

#[test]
fn update_segmentation_map_matches_c() {
    let mut rng = Lcg(0xFACE_0FF1);
    let (mi_cols, mi_rows) = (17i32, 11i32); // deliberately not SB-aligned so
    // the xmis/ymis clip at the right/bottom edge is exercised
    let base: Vec<u8> = (0..(mi_cols * mi_rows))
        .map(|_| rng.below(MAX_SEGMENTS as u32) as u8)
        .collect();
    for &bsize in BlockSize::ALL.iter() {
        let bw = (block_wide(bsize) / 4) as usize;
        let bh = (block_high(bsize) / 4) as usize;
        for mi_row in 0..mi_rows {
            for mi_col in 0..mi_cols {
                let segment_id = rng.below(MAX_SEGMENTS as u32) as u8;
                let c = cref::update_segmentation_map(
                    &base,
                    mi_cols,
                    mi_rows,
                    bsize as i32,
                    mi_row,
                    mi_col,
                    segment_id,
                );
                let mut r_map = SegmentationMap::new(mi_cols as usize, mi_rows as usize);
                r_map.data.copy_from_slice(&base);
                r_map.update(bw, bh, mi_row as usize, mi_col as usize, segment_id);
                assert_eq!(
                    c, r_map.data,
                    "update_segmentation_map bsize={bsize:?} mi=({mi_row},{mi_col}) id={segment_id}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// get_segment_id, via the exported prediction path
// ---------------------------------------------------------------------------

/// `SegmentationMap::get_segment_id` is a port of the `static INLINE`
/// `svt_aom_get_segment_id`; it has no symbol of its own, but a
/// single-nonzero-cell map makes `get_spatial_seg_prediction` return exactly
/// what the C `get_segment_id` read, so C's version is still the oracle.
#[test]
fn get_segment_id_is_pinned_through_the_prediction_path() {
    let (mi_cols, mi_rows) = (8i32, 8i32);
    for probe_row in 0..mi_rows {
        for probe_col in 0..mi_cols {
            for id in 0..MAX_SEGMENTS as u8 {
                let mut map = vec![0u8; (mi_cols * mi_rows) as usize];
                map[(probe_row * mi_cols + probe_col) as usize] = id;
                let mut r_map = SegmentationMap::new(mi_cols as usize, mi_rows as usize);
                r_map.data.copy_from_slice(&map);
                // Read the probe cell as the LEFT neighbour of (row, col+1).
                if probe_col + 1 >= mi_cols {
                    continue;
                }
                let (c_pred, c_cdf) = cref::get_spatial_seg_prediction(
                    &map,
                    mi_cols,
                    mi_rows,
                    probe_row,
                    probe_col + 1,
                    true,
                    false,
                );
                let (r_pred, r_cdf) = get_spatial_seg_prediction(
                    &r_map,
                    probe_row as usize,
                    (probe_col + 1) as usize,
                    true,
                    false,
                );
                assert_eq!((c_pred as usize, c_cdf as usize), (r_pred, r_cdf));
                assert_eq!(
                    r_map.get_segment_id(1, 1, probe_row as usize, probe_col as usize),
                    usize::from(id)
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// write_segment_id — the coded symbol itself, through the real range coder
// ---------------------------------------------------------------------------

/// Byte-for-byte against `write_segment_id` (entropy_coding.c:4867), an
/// EXPORTED symbol, driven through the real `od_ec` coder and the real
/// `FRAME_CONTEXT.seg.spatial_pred_seg_cdf`. Also compares the adapted CDF
/// and the stamped segmentation map, so `neg_interleave`, the cdf-row pick,
/// `aom_write_symbol`'s adaptation and `svt_av1_update_segmentation_map` are
/// all pinned at once.
#[test]
fn write_segment_id_matches_c() {
    let _g = fc_guard();
    let mut rng = Lcg(0x0912_3456);
    let (mi_cols, mi_rows) = (12i32, 9i32);
    let mut cases = 0usize;
    for &last_active in &[0u8, 1, 3, 7] {
        for &skip_coeff in &[false, true] {
            for trial in 0..25 {
                let map: Vec<u8> = (0..(mi_cols * mi_rows))
                    .map(|_| rng.below(u32::from(last_active) + 1) as u8)
                    .collect();
                let mi_row = rng.below(mi_rows as u32) as i32;
                let mi_col = rng.below(mi_cols as u32) as i32;
                let bsize = BlockSize::ALL[rng.below(16) as usize]; // square/2:1
                let segment_id = rng.below(u32::from(last_active) + 1) as u8;
                let left = mi_col > 0;
                let up = mi_row > 0;

                // C side: fc_init resets the process-global CDFs first, so
                // both sides start from the same defaults every iteration.
                cref::fc_init(60);
                let c = cref::write_segment_id(
                    &map,
                    mi_cols,
                    mi_rows,
                    bsize as i32,
                    mi_row,
                    mi_col,
                    left,
                    up,
                    last_active,
                    segment_id,
                    skip_coeff,
                );

                // Rust side.
                let mut fc = FrameContext::new_default();
                let mut r_map = SegmentationMap::new(mi_cols as usize, mi_rows as usize);
                r_map.data.copy_from_slice(&map);
                let mut w = svtav1_entropy::writer::AomWriter::new(1024);
                let bw = (block_wide(bsize) / 4) as usize;
                let bh = (block_high(bsize) / 4) as usize;
                let r_id = svtav1_entropy::context::write_segment_id(
                    &mut w,
                    &mut fc,
                    &mut r_map,
                    true,
                    last_active,
                    bw,
                    bh,
                    mi_row as usize,
                    mi_col as usize,
                    left,
                    up,
                    segment_id,
                    skip_coeff,
                );
                let r_bytes = w.done().to_vec();
                let r_cdf: Vec<u16> = fc.spatial_pred_seg_cdf.iter().flatten().copied().collect();

                let ctx = format!(
                    "last_active={last_active} skip={skip_coeff} trial={trial} \
                     bsize={bsize:?} mi=({mi_row},{mi_col}) id={segment_id}"
                );
                assert_eq!(c.segment_id, r_id, "returned segment_id ({ctx})");
                assert_eq!(c.seg_map, r_map.data, "segmentation map ({ctx})");
                assert_eq!(c.spatial_pred_seg_cdf, r_cdf, "adapted seg CDF ({ctx})");
                assert_eq!(c.bytes, r_bytes, "coded bytes ({ctx})");
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 200);
}

// ---------------------------------------------------------------------------
// End-to-end: the frame-header syntax the whole vertical feeds
// ---------------------------------------------------------------------------

/// `svt_aom_wb_write_inv_signed_literal` (entropy_coding.c:1377) is EXPORTED,
/// so the one nontrivial primitive inside the `static` `encode_segmentation`
/// is pinned at tier 1 even though its caller is not: this asserts the exact
/// bits for every (data, bits) pair the segmentation feature table can
/// produce.
#[test]
fn inv_signed_literal_matches_c() {
    use svtav1_entropy::obu::{BitWriter, write_segmentation_params};
    // The signed features are ALT_Q (8 bits) and the four LF deltas (6 bits).
    for &bits in &[6i32, 8] {
        let lim = 1i32 << bits; // su(1+bits) covers -lim..lim-1
        for data in -lim..lim {
            let c = cref::wb_write_inv_signed_literal(data, bits);
            assert_eq!(c.len(), (bits + 1) as usize, "bit count for bits={bits}");

            // Drive the port's writer through the only public entry point
            // that reaches it: a params struct with exactly one enabled
            // feature carrying `data`.
            let feature = if bits == 8 { 0usize } else { 1usize };
            let mut sp = SegmentationParams {
                segmentation_enabled: true,
                segmentation_update_data: true,
                ..SegmentationParams::default()
            };
            sp.feature_enabled[0][feature] = 1;
            sp.feature_data[0][feature] = data as i16;
            let mut wb = BitWriter::new();
            write_segmentation_params(&mut wb, &sp, true);
            let bits_out = decode_bits(&wb);
            // Layout: 1 enabled bit, then feature 0..feature-1 disabled
            // (one zero bit each), then the enable bit, then the payload.
            let payload_start = 1 + feature + 1;
            assert_eq!(
                &bits_out[payload_start..payload_start + c.len()],
                &c[..],
                "inv_signed_literal(data={data}, bits={bits})"
            );
        }
    }
}

/// `write_segmentation_params` fed with the EXACT `SegmentationParams` the C
/// `svt_aom_setup_segmentation` produced, so the header bits are derived from
/// C-sourced feature data rather than hand-invented values.
///
/// The writer itself (`encode_segmentation`, entropy_coding.c:2247) is
/// `static` with no symbol, so this asserts the bit LAYOUT against a
/// hand-decoded expectation rather than against C bytes — the weaker tier,
/// flagged as such in the module docs above.
#[test]
fn segmentation_params_header_from_c_derived_state() {
    use svtav1_entropy::obu::{BitWriter, write_segmentation_params};
    let mut rng = Lcg(0x7777_0001);
    let (flat, _rows) = make_variance(4, &mut rng, 3000);
    let c = cref::setup_segmentation(1, &flat, 4, seg::VARIANCE_BLOCK_COUNT as u32);
    assert!(c.enabled, "aq_mode 1 must enable segmentation in C");

    let mut sp = SegmentationParams {
        segmentation_enabled: c.enabled,
        segmentation_update_map: c.update_map,
        segmentation_temporal_update: c.temporal_update,
        segmentation_update_data: c.update_data,
        feature_data: c.feature_data,
        feature_enabled: c.feature_enabled,
        last_active_seg_id: c.last_active_seg_id,
        seg_id_pre_skip: c.seg_id_pre_skip,
        variance_bin_edge: c.variance_bin_edge,
    };

    let mut wb = BitWriter::new();
    write_segmentation_params(&mut wb, &sp, true /* PRIMARY_REF_NONE */);
    let bits = decode_bits(&wb);

    // KEY frame -> primary_ref_none -> the three update flags are NOT coded.
    // Layout: 1 enabled bit, then per (segment, feature): 1 enable bit plus
    // `feature_bits[j] + signed[j]` payload bits when enabled.
    let mut expect = Vec::new();
    expect.push(1u8); // segmentation_enabled
    for i in 0..MAX_SEGMENTS {
        for j in 0..SEG_LVL_MAX {
            let on = sp.feature_enabled[i][j] != 0;
            expect.push(u8::from(on));
            if on {
                let n = SEGMENTATION_FEATURE_BITS[j] as u32
                    + u32::from(SEGMENTATION_FEATURE_SIGNED[j] != 0);
                let v = i32::from(sp.feature_data[i][j]);
                for b in (0..n).rev() {
                    expect.push(((v >> b) & 1) as u8);
                }
            }
        }
    }
    assert_eq!(bits, expect, "segmentation_params() bit layout");
    // Only SEG_LVL_ALT_Q is on, and it is su(1+8) = 9 bits:
    // 1 + 8*8 enable bits + 8*9 payload bits.
    assert_eq!(bits.len(), 1 + 64 + 72);

    // Disabled -> exactly one zero bit, which is byte-for-byte the hardcoded
    // `wb.write_bit(false)` every current writer site emits.
    sp.segmentation_enabled = false;
    let mut wb = BitWriter::new();
    write_segmentation_params(&mut wb, &sp, true);
    assert_eq!(decode_bits(&wb), vec![0u8]);
}

/// Expand a `BitWriter`'s payload back into individual bits. The writer pads
/// the final byte with zeros, so the caller must know the expected length —
/// every use here compares against a full expectation vector.
fn decode_bits(wb: &svtav1_entropy::obu::BitWriter) -> Vec<u8> {
    let data = wb.data();
    let n = wb.bit_len();
    (0..n).map(|i| (data[i / 8] >> (7 - (i % 8))) & 1).collect()
}

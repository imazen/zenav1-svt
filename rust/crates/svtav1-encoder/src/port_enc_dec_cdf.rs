//! Port of `Codec/enc_dec_process.c`'s **per-superblock entropy-context
//! selection and CDF averaging** — how each SB decides which frame context to
//! start coding from, and how two of them are blended when both a left and a
//! top-right neighbour are available.
//!
//! **Why this matters for inter.** `cdf_ctrl.enabled` makes every SB inherit
//! adapted CDFs from its neighbours instead of restarting from the frame's
//! initial context. On a single still that is a within-frame effect; across a
//! GOP it compounds, because `md_frame_context` itself is the previous
//! frame's adapted context. Getting the SELECTION wrong shifts every symbol's
//! cost estimate, and the RD decisions with them.
//!
//! **The rule is live**: `pipeline::tile_walk::tile_body` picks each SB's
//! rate-estimation context with [`select_sb_cdf_source`] and blends with
//! the named weights. The blend itself is
//! [`crate::entropy::context::FrameContext::avg_cdf_with`] over
//! [`crate::entropy::cdf::avg_cdf_entries`], which owns the field
//! enumeration and is tested against C's `avg_cdf_symbol` formula.
//!
//! **Evidence.** `avg_cdf_symbol` / `avg_cdf_symbols` survive C's Release
//! build only with both weights constant-propagated out, so they cannot be
//! bound (`svtav1-cref/build.rs` `link_globalized_enc_dec_statics` refuses to
//! promote them). The selection rule is checked end to end instead:
//! `tools/tile_gate.sh` pins multi-tile-row and multi-tile-column cells at
//! allintra preset 6 (`update_cdf_level` 2, so `cdf_ctrl.update_se` is on
//! and this chain prices every SB) IDENTICAL to C, including the 512x384
//! and 640x448 ragged geometries.

/// C `AVG_CDF_WEIGHT_LEFT` (enc_dec_process.c:2540). The LEFT neighbour is
/// weighted 3x — it is the more recently adapted of the two.
pub const AVG_CDF_WEIGHT_LEFT: i32 = 3;
/// C `AVG_CDF_WEIGHT_TOP` (enc_dec_process.c:2541).
pub const AVG_CDF_WEIGHT_TOP: i32 = 1;

/// Where a superblock's starting entropy context comes from.
///
/// C expresses this as four assignments into `pcs->ec_ctx_array[sb_index]`
/// plus one conditional `avg_cdf_symbols` call
/// (enc_dec_process.c:2866-2899). Naming the outcomes makes the
/// `!left && !top_right` / `!left` / `!top_right` / both ladder checkable, and
/// makes the "copy then blend" shape of the last arm explicit — C copies the
/// LEFT context in first and then averages the top-right INTO it, so the
/// left neighbour is both the base and the 3x-weighted term.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SbCdfSource {
    /// `pcs->md_frame_context` — the frame's initial (previous-frame-adapted)
    /// context.
    FrameContext,
    /// `ec_ctx_array[sb_index - 1]` — the SB to the left.
    Left,
    /// `ec_ctx_array[sb_index - pic_width_in_sb + 1]` — the SB above-right.
    TopRight,
    /// Copy `Left`, then `avg_cdf_symbols(&it, &top_right, 3, 1)`.
    LeftBlendedWithTopRight,
}

/// The picture-level knobs `select_sb_cdf_source` reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct SbCdfConfig {
    /// `pcs->cdf_ctrl.enabled` — 1 if mv, se or coeff CDF update is on.
    pub cdf_enabled: bool,
    /// `scs->pic_based_rate_est`.
    pub pic_based_rate_est: bool,
    /// `scs->enc_dec_segment_row_count_array`.
    pub segment_rows: u32,
    /// `scs->enc_dec_segment_col_count_array`.
    pub segment_cols: u32,
}

/// C's per-SB context choice (enc_dec_process.c:2866-2899).
///
/// `None` means `cdf_ctrl.enabled` is off, in which case C touches
/// `ec_ctx_array` not at all — distinct from choosing
/// [`SbCdfSource::FrameContext`], which OVERWRITES it.
///
/// The `pic_based_rate_est` arm is the serial one: with a single enc-dec
/// segment the SBs are coded in raster order by one thread, so SB *n* can
/// simply take SB *n-1*'s context and no neighbour test is needed. Everything
/// else uses the availability ladder, whose tests are in MI units against the
/// TILE bounds — an SB at a tile's left edge has no left neighbour even when
/// it has one in the picture.
#[must_use]
pub fn select_sb_cdf_source(
    cfg: &SbCdfConfig,
    sb_index: u32,
    sb_origin_x: u32,
    sb_origin_y: u32,
    sb_size_log2: u32,
    tile_mi_row_start: i32,
    tile_mi_col_start: i32,
    tile_mi_col_end: i32,
) -> Option<SbCdfSource> {
    /// C `MI_SIZE_LOG2` (definitions.h) — MI units are 4x4 luma samples.
    const MI_SIZE_LOG2: u32 = 2;

    if !cfg.cdf_enabled {
        return None;
    }
    if cfg.pic_based_rate_est && cfg.segment_rows == 1 && cfg.segment_cols == 1 {
        return Some(if sb_index == 0 {
            SbCdfSource::FrameContext
        } else {
            SbCdfSource::Left
        });
    }

    let top_right_available = ((sb_origin_y >> MI_SIZE_LOG2) as i32 > tile_mi_row_start)
        && (((sb_origin_x + (1 << sb_size_log2)) >> MI_SIZE_LOG2) as i32) < tile_mi_col_end;
    let left_available = (sb_origin_x >> MI_SIZE_LOG2) as i32 > tile_mi_col_start;

    Some(match (left_available, top_right_available) {
        (false, false) => SbCdfSource::FrameContext,
        (false, true) => SbCdfSource::TopRight,
        (true, false) => SbCdfSource::Left,
        (true, true) => SbCdfSource::LeftBlendedWithTopRight,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **EVIDENCE TIER 4** for every test here — see the module header for
    /// why tier 1 is not reachable (both C symbols have a constant-propagated
    /// ABI). Vectors are hand-derived from the C source at the cited lines.
    const _: () = ();

    fn cfg(enabled: bool, pic_based: bool, rows: u32, cols: u32) -> SbCdfConfig {
        SbCdfConfig {
            cdf_enabled: enabled,
            pic_based_rate_est: pic_based,
            segment_rows: rows,
            segment_cols: cols,
        }
    }

    /// `cdf_ctrl.enabled == 0` means C does not write `ec_ctx_array` AT ALL,
    /// which is not the same as selecting the frame context.
    #[test]
    fn select_sb_cdf_source_off_is_not_frame_context() {
        assert_eq!(
            select_sb_cdf_source(&cfg(false, false, 4, 4), 5, 128, 128, 6, 0, 0, 1000),
            None
        );
    }

    /// The serial (`pic_based_rate_est` + one segment) arm ignores the
    /// neighbour tests entirely and chains SB to SB.
    #[test]
    fn select_sb_cdf_source_serial_arm_chains_left() {
        let c = cfg(true, true, 1, 1);
        assert_eq!(
            select_sb_cdf_source(&c, 0, 0, 0, 6, 0, 0, 1000),
            Some(SbCdfSource::FrameContext)
        );
        // SB 1 takes SB 0 even though it sits at the tile's left edge, where
        // the availability ladder would have said FrameContext.
        assert_eq!(
            select_sb_cdf_source(&c, 1, 0, 64, 6, 0, 0, 1000),
            Some(SbCdfSource::Left)
        );
    }

    /// The availability ladder, in MI units against the TILE bounds.
    #[test]
    fn select_sb_cdf_source_availability_ladder() {
        let c = cfg(true, false, 4, 4);
        // 64x64 SBs (log2 = 6); a tile starting at MI (0, 0) and ending at
        // MI column 32 (i.e. 128 luma samples wide).
        let pick = |x: u32, y: u32, col_end: i32| {
            select_sb_cdf_source(&c, 0, x, y, 6, 0, 0, col_end).unwrap()
        };
        // Top-left SB: no left (x == tile start), no top-right (y == row
        // start).
        assert_eq!(pick(0, 0, 32), SbCdfSource::FrameContext);
        // First SB of the second row: still no left, but the SB above-right
        // exists.
        assert_eq!(pick(0, 64, 32), SbCdfSource::TopRight);
        // Second SB of the first row: left exists, no row above.
        assert_eq!(pick(64, 0, 32), SbCdfSource::Left);
        // Interior of a 3-SB-wide tile (MI column end 48): both available.
        assert_eq!(pick(64, 64, 48), SbCdfSource::LeftBlendedWithTopRight);
        // At the tile's RIGHT edge the top-right is unavailable even with a
        // row above, because the test is `x + sb_size < col_end` and NOT
        // `<=` — the SB whose right edge lands exactly on the tile boundary
        // already has no above-right neighbour. This vector was wrong in a
        // first draft (it expected a blend at col_end == 32, where the SB at
        // x = 64 IS the last column) and the failure is the reason the
        // strictness of that comparison is called out here.
        assert_eq!(pick(64, 64, 32), SbCdfSource::Left);
        assert_eq!(pick(64, 64, 16), SbCdfSource::Left);
    }
}

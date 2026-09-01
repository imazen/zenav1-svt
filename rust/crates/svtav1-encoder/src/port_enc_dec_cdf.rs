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
//! **EVIDENCE: TIER 4 for everything here, and the reason is worth reading
//! before someone tries to improve it.** `avg_cdf_symbol` (:2543) and
//! `avg_cdf_symbols` (:2585) DO survive the Release build as local symbols —
//! but their only call site passes the literal weights
//! `AVG_CDF_WEIGHT_LEFT = 3` / `AVG_CDF_WEIGHT_TOP = 1` (:2540-2541, :2895),
//! and LLVM constant-propagated both parameters out of both signatures. The
//! compiled `avg_cdf_symbols` overwrites `w2`/`w3` (its own `wt_left` /
//! `wt_tr`) before its first call and never stages them anywhere. Binding
//! either as declared would pass garbage in the weight slots — the same trap
//! that produced a wrong 10-bit SSIM in
//! [`crate::port_enc_dec_metrics`]. `svtav1-cref/build.rs`
//! (`link_globalized_enc_dec_statics`) records the disassembly and REFUSES to
//! promote them.
//!
//! So these are hand-derived vectors traced against the C source, and they say
//! so. A future session that wants tier 1 here needs either a shim that
//! reaches them through the exported `svt_aom_mode_decision_kernel` (i.e. a
//! whole encode) or a C build without IPO.
//!
//! **SCOPE, stated because the missing part is larger than the present part.**
//! [`avg_cdf_symbol`] is the primitive, and [`SbCdfSource`] /
//! [`select_sb_cdf_source`] are the selection policy. The full
//! `avg_cdf_symbols` / `avg_nmv` field enumeration — sixty-odd CDF arrays
//! walked in a fixed order — is NOT here: it belongs with whoever owns the
//! crate's `FrameContext` type, and duplicating that enumeration in this lane
//! would create two lists that must agree. [`AvgCdfPlan`] is the shape such a
//! port should drive this primitive with, and the weights are named
//! constants so the caller cannot guess them.

/// C `AVG_CDF_WEIGHT_LEFT` (enc_dec_process.c:2540). The LEFT neighbour is
/// weighted 3x — it is the more recently adapted of the two.
pub const AVG_CDF_WEIGHT_LEFT: i32 = 3;
/// C `AVG_CDF_WEIGHT_TOP` (enc_dec_process.c:2541).
pub const AVG_CDF_WEIGHT_TOP: i32 = 1;

/// C `avg_cdf_symbol` (enc_dec_process.c:2543).
///
/// A rounded weighted average of two CDF tables, IN PLACE into `left`.
///
/// Three details that a natural rewrite gets wrong:
///
/// * **The inner loop is `j <= nsymbs`, not `j < nsymbs`.** An AV1 CDF array
///   of `n` symbols has `n + 1` entries — `n` probabilities plus the trailing
///   adaptation counter — and the counter is averaged along with them. Using
///   `<` would leave the counter unblended and slowly desynchronise the
///   adaptation rate.
/// * **`cdf_stride` is not `nsymbs + 1`.** C's `AVG_CDF_STRIDE` macro derives
///   `num_cdfs` from `sizeof(array) / cdf_stride` and passes `CDF_SIZE(nsymbs)`
///   as the stride, which is `nsymbs + 1` ROUNDED UP for alignment in several
///   tables. Rows are therefore strided, and entries past `nsymbs` in each row
///   are skipped.
/// * **The rounding is `+ (wt_left + wt_tr) / 2` before an integer divide**,
///   i.e. round-half-up on a non-negative numerator — not a shift, and not
///   round-to-even.
///
/// The arithmetic is done in `i32` because C's is: each CDF entry is a
/// `uint16_t` promoted to `int`, and the products cannot exceed
/// `32768 * 3 + 32768 * 1`.
///
/// # Panics
/// When `left` or `tr` is shorter than `num_cdfs * cdf_stride`, or when
/// `wt_left + wt_tr` is zero (C would divide by zero).
pub fn avg_cdf_symbol(
    left: &mut [u16],
    tr: &[u16],
    num_cdfs: usize,
    cdf_stride: usize,
    nsymbs: usize,
    wt_left: i32,
    wt_tr: i32,
) {
    let total = wt_left + wt_tr;
    assert!(total != 0, "avg_cdf_symbol: zero total weight");
    assert!(nsymbs < cdf_stride, "avg_cdf_symbol: nsymbs >= cdf_stride");
    assert!(left.len() >= num_cdfs * cdf_stride);
    assert!(tr.len() >= num_cdfs * cdf_stride);
    for i in 0..num_cdfs {
        let base = i * cdf_stride;
        // `j <= nsymbs`: the trailing adaptation counter is averaged too.
        for j in 0..=nsymbs {
            let l = i32::from(left[base + j]);
            let t = i32::from(tr[base + j]);
            left[base + j] = ((l * wt_left + t * wt_tr + total / 2) / total) as u16;
        }
    }
}

/// One entry of the table C's `AVERAGE_CDF` / `AVG_CDF_STRIDE` macros expand
/// to: a CDF array, how many symbols it codes, and its row stride.
///
/// C derives `num_cdfs` at each macro expansion from `sizeof(array) /
/// cdf_stride`, so the count is a property of the array and not something a
/// caller supplies. A port of the full `avg_cdf_symbols` should compute it the
/// same way from its own array lengths rather than hard-coding sixty numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvgCdfPlan {
    /// C `nsymbs`.
    pub nsymbs: usize,
    /// C `cdf_stride`, i.e. `CDF_SIZE(nsymbs)` at most call sites but NOT at
    /// all of them — several tables pass an explicit larger stride.
    pub cdf_stride: usize,
}

impl AvgCdfPlan {
    /// C `AVERAGE_CDF(l, r, nsymbs)`, whose stride is `CDF_SIZE(nsymbs)`.
    #[must_use]
    pub const fn average_cdf(nsymbs: usize) -> Self {
        Self {
            nsymbs,
            cdf_stride: nsymbs + 1,
        }
    }

    /// C `AVG_CDF_STRIDE(l, r, nsymbs, cdf_stride)`.
    #[must_use]
    pub const fn with_stride(nsymbs: usize, cdf_stride: usize) -> Self {
        Self { nsymbs, cdf_stride }
    }

    /// C's `num_cdfs = array_size / cdf_stride`.
    #[must_use]
    pub const fn num_cdfs(&self, array_len: usize) -> usize {
        array_len / self.cdf_stride
    }

    /// Apply this plan to one array pair with the encoder's weights.
    pub fn apply(&self, left: &mut [u16], tr: &[u16]) {
        let n = self.num_cdfs(left.len().min(tr.len()));
        avg_cdf_symbol(
            left,
            tr,
            n,
            self.cdf_stride,
            self.nsymbs,
            AVG_CDF_WEIGHT_LEFT,
            AVG_CDF_WEIGHT_TOP,
        );
    }
}

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

/// C `copy_mv_rate` (enc_dec_process.c:36).
///
/// Copies the MV rate tables from the picture's shared estimator into a
/// per-thread one, and then re-points two stack pointers into the copy.
///
/// **Only ONE of the two cost tables is copied**, selected by
/// `allow_high_precision_mv` — the other is left holding whatever the
/// destination had. That is safe in C only because the stack pointers below
/// select the same table, so nothing reads the stale one; a port that copied
/// both "to be safe" would be doing more work for no observable difference,
/// and one that copied the wrong one would be silently wrong. Returning which
/// table is live makes the coupling explicit.
///
/// The pointer re-pointing itself (`nmvcoststack[i] = &table[i][MV_MAX]`) is
/// C's way of giving the cost lookup a zero-centred index; a Rust port
/// indexes `table[i][MV_MAX + mv]` instead and needs no pointers, so this
/// function returns the SELECTION and leaves the indexing to the caller.
#[must_use]
pub fn copy_mv_rate(allow_high_precision_mv: bool) -> MvRateTable {
    if allow_high_precision_mv {
        MvRateTable::HighPrecision
    } else {
        MvRateTable::Regular
    }
}

/// Which of `MdRateEstimationContext`'s two MV cost tables is live for this
/// frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MvRateTable {
    /// `nmv_costs`, used when `allow_high_precision_mv` is 0.
    Regular,
    /// `nmv_costs_hp`, used when it is 1.
    HighPrecision,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **EVIDENCE TIER 4** for every test here — see the module header for
    /// why tier 1 is not reachable (both C symbols have a constant-propagated
    /// ABI). Vectors are hand-derived from the C source at the cited lines.
    const _: () = ();

    /// `avg_cdf_symbol` (:2543): the rounded 3:1 blend, and the `j <= nsymbs`
    /// bound that includes the trailing adaptation counter.
    #[test]
    fn avg_cdf_symbol_blends_three_to_one_and_includes_the_counter() {
        // One CDF of 2 symbols: stride 3, entries [p0, p1, counter].
        let mut left = vec![1000u16, 2000, 4];
        let tr = vec![2000u16, 6000, 8];
        avg_cdf_symbol(&mut left, &tr, 1, 3, 2, 3, 1);
        // (1000*3 + 2000*1 + 2) / 4 == 1250
        assert_eq!(left[0], 1250);
        // (2000*3 + 6000*1 + 2) / 4 == 3000
        assert_eq!(left[1], 3000);
        // The COUNTER is averaged too: (4*3 + 8*1 + 2) / 4 == 5
        assert_eq!(left[2], 5);
    }

    /// The rounding is `+ total/2` then truncating divide — round-half-up,
    /// not a shift and not round-to-even.
    #[test]
    fn avg_cdf_symbol_rounds_half_up() {
        // (1*3 + 3*1 + 2) / 4 == 2  (exact 1.5 rounds UP)
        let mut left = vec![1u16, 0];
        let tr = vec![3u16, 0];
        avg_cdf_symbol(&mut left, &tr, 1, 2, 0, 3, 1);
        assert_eq!(left[0], 2);
        // (2*3 + 3*1 + 2) / 4 == 2  (2.25 rounds DOWN)
        let mut left = vec![2u16, 0];
        let tr = vec![3u16, 0];
        avg_cdf_symbol(&mut left, &tr, 1, 2, 0, 3, 1);
        assert_eq!(left[0], 2);
    }

    /// Entries past `nsymbs` in a strided row are NOT touched — the stride can
    /// exceed `nsymbs + 1`.
    #[test]
    fn avg_cdf_symbol_skips_the_stride_padding() {
        // Two CDFs of 1 symbol at stride 4: entries 0,1 blended; 2,3 padding.
        let mut left = vec![100u16, 4, 7777, 8888, 200, 6, 9999, 1111];
        let tr = vec![200u16, 8, 0, 0, 400, 10, 0, 0];
        avg_cdf_symbol(&mut left, &tr, 2, 4, 1, 3, 1);
        assert_eq!(left[0], (100 * 3 + 200 + 2) / 4);
        assert_eq!(left[1], (4 * 3 + 8 + 2) / 4);
        assert_eq!(left[2], 7777, "stride padding must be untouched");
        assert_eq!(left[3], 8888, "stride padding must be untouched");
        assert_eq!(left[4], (200 * 3 + 400 + 2) / 4);
        assert_eq!(left[6], 9999);
    }

    /// `AvgCdfPlan::num_cdfs` reproduces C's `sizeof(array) / cdf_stride`.
    #[test]
    fn avg_cdf_plan_derives_the_count_from_the_array() {
        let p = AvgCdfPlan::average_cdf(3); // stride 4
        assert_eq!(p.cdf_stride, 4);
        assert_eq!(p.num_cdfs(40), 10);
        let q = AvgCdfPlan::with_stride(3, 8);
        assert_eq!(q.num_cdfs(40), 5);
    }

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

    /// `copy_mv_rate` (:36) copies exactly ONE of the two tables.
    #[test]
    fn copy_mv_rate_selects_one_table() {
        assert_eq!(copy_mv_rate(true), MvRateTable::HighPrecision);
        assert_eq!(copy_mv_rate(false), MvRateTable::Regular);
    }
}

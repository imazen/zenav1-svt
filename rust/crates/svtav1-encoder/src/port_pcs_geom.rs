//! Block-grid geometry and allocation sizing from `Codec/pcs.c`.
//!
//! | Rust | C (`Codec/pcs.c`) |
//! |---|---|
//! | [`b64_grid`] / [`b64_geom`] / [`fill_b64_geoms`] | `b64_geom_init` (1491) — **EXPORTED** |
//! | [`sb_grid`] / [`sb_geom`] / [`fill_sb_geoms`] | `sb_geom_init` (1535) — **EXPORTED** |
//! | [`copy_sb_geoms`] | `copy_sb_geoms` (1528) — EXPORTED |
//! | [`max_allocated_me_refs`] | `svt_aom_get_max_allocated_me_refs` (88) — **EXPORTED** |
//! | [`out_buffer_size`] | `svt_aom_get_out_buffer_size` (374) — **EXPORTED** |
//! | [`restoration_unit_sizes`] | `set_restoration_unit_size` (29) — static |
//!
//! # Why the geometry matters beyond "it is a loop bound"
//!
//! [`B64Geom::is_complete_b64`] gates a large amount of the encoder: the ME
//! search, the pre-analysis statistics, the screen-content detector and the
//! temporal filter all take a different path on a partial 64x64 block. Getting
//! the flag wrong on ONE edge block is a different mode decision on that
//! block, not a crash — so it is worth having one definition of it.
//!
//! # Not ported here, and why
//!
//! `alloc_sb_geoms` / `free_sb_geoms` (`pcs.c:1515`, `:1522`) are the
//! `EB_MALLOC_ARRAY` / `EB_FREE_ARRAY` pair around the array these functions
//! fill; a Rust caller owns its own storage, so there is nothing to translate.
//! The [`fill_b64_geoms`] / [`fill_sb_geoms`] pair takes the destination as a
//! slice for the same reason: it keeps the module allocation-free, so it works
//! under `no_std` and lets the caller decide between a `Vec` and a
//! stack array.
//!
//! # Evidence
//!
//! Tier 1 — all four of the functions with real content are exported
//! (`nm -g Bin/Release/libSvtAv1Enc.a` reports `T _b64_geom_init`,
//! `T _sb_geom_init`, `T _svt_aom_get_max_allocated_me_refs`,
//! `T _svt_aom_get_out_buffer_size`). See
//! `tests/c_parity_pcs_geom.rs`.

/// C `B64Geom` (`Codec/pcs.h:406-412`) — one 64x64 base block's placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct B64Geom {
    /// C `org_x` — luma column of the block's left edge.
    pub org_x: u16,
    /// C `org_y` — luma row of the block's top edge.
    pub org_y: u16,
    /// C `width` — clipped to the picture, so an edge block is narrower.
    pub width: u8,
    /// C `height` — clipped to the picture.
    pub height: u8,
    /// C `is_complete_b64` — the block is a full `b64_size` square.
    pub is_complete_b64: bool,
}

/// C `SbGeom` (`Codec/pcs.h:414-419`) — one superblock's placement.
///
/// The same shape as [`B64Geom`] without the completeness flag, because the
/// superblock grid's partial blocks are handled by the partition search rather
/// than by a per-block predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SbGeom {
    /// C `org_x`.
    pub org_x: u16,
    /// C `org_y`.
    pub org_y: u16,
    /// C `width`.
    pub width: u8,
    /// C `height`.
    pub height: u8,
}

/// C `DIVIDE_AND_CEIL` on the 64x64 base-block grid (`pcs.c:1496-1497`).
///
/// Returns `(cols, rows)`. A zero dimension gives a zero count, which is what
/// C's `(x + n - 1) / n` produces.
#[must_use]
pub fn b64_grid(width: u16, height: u16, b64_size: u8) -> (u16, u16) {
    grid(width, height, u16::from(b64_size))
}

/// C `DIVIDE_AND_CEIL` on the superblock grid (`pcs.c:1536-1537`).
#[must_use]
pub fn sb_grid(width: u16, height: u16, sb_size: u16) -> (u16, u16) {
    grid(width, height, sb_size)
}

fn grid(width: u16, height: u16, unit: u16) -> (u16, u16) {
    debug_assert!(unit != 0, "a zero block size has no grid");
    (width.div_ceil(unit), height.div_ceil(unit))
}

/// One entry of C `b64_geom_init`'s loop (`pcs.c:1500-1510`).
///
/// `index` is raster order over the grid [`b64_grid`] returns.
///
/// Width and height are C's `MIN(picture - origin, b64_size)` cast to `u8`.
/// `b64_size` is a `uint8_t` in C, so the cast cannot truncate a complete
/// block; a partial block is smaller still.
#[must_use]
pub fn b64_geom(width: u16, height: u16, b64_size: u8, index: u32) -> B64Geom {
    let (cols, _) = b64_grid(width, height, b64_size);
    let unit = u16::from(b64_size);
    let org_x = (index % u32::from(cols)) as u16 * unit;
    let org_y = (index / u32::from(cols)) as u16 * unit;
    let w = width.saturating_sub(org_x).min(unit) as u8;
    let h = height.saturating_sub(org_y).min(unit) as u8;
    B64Geom {
        org_x,
        org_y,
        width: w,
        height: h,
        is_complete_b64: w == b64_size && h == b64_size,
    }
}

/// One entry of C `sb_geom_init`'s loop (`pcs.c:1545-1554`).
///
/// C stores `MIN(width - org_x, scs->sb_size)` into a `uint8_t`. The largest
/// superblock is 128, which FITS, so nothing truncates at any size C ships —
/// checked over the full dimension sweep in `tests/c_parity_pcs_geom.rs`
/// rather than assumed. The `as u8` is C's store and is kept so a future
/// `sb_size` above 255 would diverge the same way C does.
#[must_use]
pub fn sb_geom(width: u16, height: u16, sb_size: u16, index: u32) -> SbGeom {
    let (cols, _) = sb_grid(width, height, sb_size);
    let org_x = (index % u32::from(cols)) as u16 * sb_size;
    let org_y = (index / u32::from(cols)) as u16 * sb_size;
    SbGeom {
        org_x,
        org_y,
        width: width.saturating_sub(org_x).min(sb_size) as u8,
        height: height.saturating_sub(org_y).min(sb_size) as u8,
    }
}

/// C `b64_geom_init` (`pcs.c:1491`) — **EXPORTED**, minus the allocation.
///
/// `out` must be exactly `cols * rows` long; the caller owns the storage.
///
/// # Panics
///
/// If `out.len()` is not the grid size, which is a caller bug rather than a
/// runtime condition.
pub fn fill_b64_geoms(width: u16, height: u16, b64_size: u8, out: &mut [B64Geom]) {
    let (cols, rows) = b64_grid(width, height, b64_size);
    assert_eq!(
        out.len(),
        usize::from(cols) * usize::from(rows),
        "the destination must be exactly the 64x64 grid"
    );
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = b64_geom(width, height, b64_size, i as u32);
    }
}

/// C `sb_geom_init` (`pcs.c:1535`) — **EXPORTED**, minus the allocation.
///
/// # Panics
///
/// If `out.len()` is not the grid size.
pub fn fill_sb_geoms(width: u16, height: u16, sb_size: u16, out: &mut [SbGeom]) {
    let (cols, rows) = sb_grid(width, height, sb_size);
    assert_eq!(
        out.len(),
        usize::from(cols) * usize::from(rows),
        "the destination must be exactly the superblock grid"
    );
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = sb_geom(width, height, sb_size, i as u32);
    }
}

/// C `copy_sb_geoms` (`pcs.c:1528`) — EXPORTED.
///
/// A `memcpy` in all but name; kept as a named function only so the port map
/// has a counterpart and a reader is not left wondering where it went.
///
/// # Panics
///
/// If the two slices differ in length.
pub fn copy_sb_geoms(dst: &mut [SbGeom], src: &[SbGeom]) {
    assert_eq!(dst.len(), src.len());
    dst.copy_from_slice(src);
}

/// C `svt_aom_get_max_allocated_me_refs` (`pcs.c:88`) — **EXPORTED**.
///
/// Returns `(max_ref_to_alloc, max_cand_to_alloc)`: how many motion vectors
/// and how many ME candidates one prediction unit can produce, given the
/// reference counts the two lists will actually signal.
///
/// The candidate count is every single reference, plus every list0 x list1
/// bi-directional pair, plus the `list0 - 1` uni-directional list-0 compounds,
/// plus ONE more when list 1 has all three references — the
/// `BWDREF + ALTREF` uni-directional compound, which only exists at that
/// count.
///
/// Integer note, and it is part of the contract rather than a detail: C
/// promotes both `uint8_t` arguments to `int`, evaluates the whole expression
/// SIGNED, and stores each result back into a `uint8_t`. Two consequences a
/// differential over the full `u8 x u8` domain sees:
///
/// * `ref_count_used_list0 - 1` is **-1** at `list0 == 0`, and `-1` stored to
///   a `uint8_t` is 255 — not a saturating 0;
/// * both results truncate, so `(255, 255)` gives `max_ref = 254`, not 255.
///
/// So the arithmetic here is `i32` with an explicit `as u8` store, matching C
/// exactly. Widening to `u32` and saturating would agree on every reachable
/// input — the largest real counts are (4, 3), giving 7 and 23 — and differ
/// on both corners above.
#[must_use]
pub fn max_allocated_me_refs(ref_count_used_list0: u8, ref_count_used_list1: u8) -> (u8, u8) {
    let l0 = i32::from(ref_count_used_list0);
    let l1 = i32::from(ref_count_used_list1);
    let max_ref = l0 + l1;
    let max_cand = l0 + l1 + (l0 * l1) + (l0 - 1) + i32::from(l1 == 3);
    (max_ref as u8, max_cand as u8)
}

/// C `svt_aom_get_out_buffer_size` (`pcs.c:374`) — **EXPORTED**.
///
/// The initial bitstream capacity: a quarter of the raw 4:2:0 frame. The
/// entropy writer grows on demand, so this is a starting size and not a limit.
///
/// Integer note: C evaluates `picture_width * picture_height * 3 / 2 / 4` in
/// `uint32_t`, so it WRAPS above roughly 2.86 gigapixels. Reproduced with
/// `wrapping_mul`, because a `u64` intermediate would give a different answer
/// exactly where C's differs, and the differential covers that range.
#[must_use]
pub fn out_buffer_size(picture_width: u32, picture_height: u32) -> u32 {
    picture_width.wrapping_mul(picture_height).wrapping_mul(3) / 2 / 4
}

/// C `set_restoration_unit_size` (`pcs.c:29`) — static.
///
/// Returns the loop-restoration unit size for the three planes.
///
/// Every parameter C takes is `(void)`-cast away and the shift `s` is a
/// literal 0, so all three planes get `RESTORATION_UNITSIZE_MAX` and the
/// function is a constant. That is worth stating rather than deleting: the
/// dead parameters are where a chroma-subsampled unit size WOULD be derived,
/// and a reader who assumes the port simplified something needs to see that
/// there was nothing to simplify.
#[must_use]
pub fn restoration_unit_sizes() -> [i32; 3] {
    use svtav1_dsp::restoration::RESTORATION_UNITSIZE_MAX;
    [
        RESTORATION_UNITSIZE_MAX,
        RESTORATION_UNITSIZE_MAX,
        RESTORATION_UNITSIZE_MAX,
    ]
}

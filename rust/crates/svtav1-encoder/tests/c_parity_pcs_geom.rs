//! `Codec/pcs.c` block-grid geometry and allocation sizing against the REAL
//! exported C symbols — evidence **tier 1**
//! (`docs/WORKING-ON-THIS.md` §4).
//!
//! `nm -g Bin/Release/libSvtAv1Enc.a` reports `T _b64_geom_init`,
//! `T _sb_geom_init`, `T _svt_aom_get_max_allocated_me_refs` and
//! `T _svt_aom_get_out_buffer_size`, so every function with real content in
//! this group has a symbol and none of it needs a hand-derived vector.

use svtav1_cref::ref_mgmt as cref;
use svtav1_encoder::port_pcs_geom as geom;

/// TIER 1. `b64_geom_init` over a dimension grid chosen to hit every edge
/// case the loop has: exact multiples, one pixel over, one pixel under, and
/// the sub-block frames where the whole picture is one partial block.
///
/// The interesting output is `is_complete_b64`, which gates the ME search,
/// the pre-analysis statistics, the screen-content detector and the temporal
/// filter — a wrong flag on one edge block is a different mode decision there.
#[test]
fn c_parity_b64_geom_init() {
    let dims = [
        1u16, 2, 7, 8, 15, 16, 31, 32, 63, 64, 65, 96, 127, 128, 129, 191, 192, 200, 255, 256, 257,
        320, 511, 512, 513, 640, 720, 1024, 1080, 1920,
    ];
    let mut cells = 0usize;
    let mut complete = 0usize;
    let mut partial = 0usize;

    for b64_size in [16u8, 32, 64] {
        for &w in &dims {
            for &h in &dims {
                let want = cref::b64_geom_init(b64_size, w, h);
                let (cols, rows) = geom::b64_grid(w, h, b64_size);
                let n = usize::from(cols) * usize::from(rows);
                if n == 0 || n > 4096 {
                    continue;
                }
                let mut got = vec![geom::B64Geom::default(); n];
                geom::fill_b64_geoms(w, h, b64_size, &mut got);
                assert_eq!(want.len(), n, "b64 {b64_size} {w}x{h}: grid size");
                for (i, &(x, y, gw, gh, c)) in want.iter().enumerate() {
                    assert_eq!(
                        got[i],
                        geom::B64Geom {
                            org_x: x,
                            org_y: y,
                            width: gw,
                            height: gh,
                            is_complete_b64: c
                        },
                        "b64 {b64_size} {w}x{h} block {i}"
                    );
                    if c { complete += 1 } else { partial += 1 }
                }
                cells += 1;
            }
        }
    }
    // Anti-vacuity: the sweep must actually have produced both kinds of block,
    // or the flag that matters was never exercised.
    assert!(cells > 2000, "only {cells} grids compared");
    assert!(
        complete > 0 && partial > 0,
        "{complete} complete / {partial} partial"
    );
}

/// TIER 1. `sb_geom_init` over the same grid, at both superblock sizes.
///
/// The 128 case is the one worth having, and it settles a question rather than
/// assuming an answer: C stores `MIN(width - org_x, sb_size)` into a
/// `uint8_t`, so it is natural to expect a complete 128-wide superblock to
/// truncate. It does NOT — 128 fits in a byte — and this sweep is what shows
/// that, by requiring a `width == 128` block to appear.
#[test]
fn c_parity_sb_geom_init() {
    let dims = [
        1u16, 8, 63, 64, 65, 127, 128, 129, 192, 255, 256, 257, 384, 512, 640, 720, 1024, 1080,
        1920,
    ];
    let mut saw_full_128 = false;
    for sb_size in [64u16, 128] {
        for &w in &dims {
            for &h in &dims {
                let want = cref::sb_geom_init(sb_size, w, h);
                let (cols, rows) = geom::sb_grid(w, h, sb_size);
                let n = usize::from(cols) * usize::from(rows);
                if n == 0 || n > 4096 {
                    continue;
                }
                let mut got = vec![geom::SbGeom::default(); n];
                geom::fill_sb_geoms(w, h, sb_size, &mut got);
                assert_eq!(want.len(), n, "sb {sb_size} {w}x{h}: grid size");
                for (i, &(x, y, gw, gh)) in want.iter().enumerate() {
                    assert_eq!(
                        got[i],
                        geom::SbGeom {
                            org_x: x,
                            org_y: y,
                            width: gw,
                            height: gh
                        },
                        "sb {sb_size} {w}x{h} block {i}"
                    );
                    saw_full_128 |= sb_size == 128 && gw == 128 && gh == 128;
                }
            }
        }
    }
    assert!(
        saw_full_128,
        "no complete 128x128 superblock was produced, so the uint8_t store was never exercised at its widest"
    );
}

/// TIER 1. `svt_aom_get_max_allocated_me_refs` over its ENTIRE input domain —
/// both arguments are `uint8_t`, so 65,536 cells.
///
/// The out-of-envelope inputs are the point. C computes the candidate total in
/// `int` and stores to `uint8_t`, and `ref_count_used_list0 - 1` underflows at
/// list0 = 0; a port that widened to `u32` and clamped, or that used
/// saturating arithmetic, would agree on every reachable input and diverge
/// exactly there.
#[test]
fn c_parity_max_allocated_me_refs_full_domain() {
    let mut distinct = std::collections::HashSet::new();
    for l0 in 0u8..=255 {
        for l1 in 0u8..=255 {
            let want = cref::max_allocated_me_refs(l0, l1);
            let got = geom::max_allocated_me_refs(l0, l1);
            assert_eq!(got, want, "l0={l0} l1={l1}");
            distinct.insert(want);
        }
    }
    assert!(
        distinct.len() > 100,
        "only {} distinct results",
        distinct.len()
    );
    // The reachable corner, stated so the domain sweep is not the only record:
    // 4 list-0 and 3 list-1 references give 4+3 MVs and
    // 4+3+12+3+1 = 23 candidates.
    assert_eq!(geom::max_allocated_me_refs(4, 3), (7, 23));
}

/// TIER 1. `svt_aom_get_out_buffer_size`, including the range where C's
/// `uint32_t` product WRAPS.
///
/// A `u64` intermediate would be "more correct" and would disagree with the
/// shipping library above about 2.86 gigapixels, so the port wraps on purpose
/// and this drives both sides of the boundary.
#[test]
fn c_parity_out_buffer_size() {
    let dims = [
        0u32, 1, 2, 16, 64, 176, 352, 640, 720, 1280, 1920, 3840, 7680, 15360, 30720, 65535, 65536,
        100_000, 1_000_000,
    ];
    let mut wrapped = 0usize;
    for &w in &dims {
        for &h in &dims {
            let want = cref::out_buffer_size(w, h);
            let got = geom::out_buffer_size(w, h);
            assert_eq!(got, want, "{w}x{h}");
            if u64::from(w) * u64::from(h) * 3 > u64::from(u32::MAX) {
                wrapped += 1;
            }
        }
    }
    assert!(wrapped > 0, "the sweep never reached the wrapping range");
    // A 1080p frame: 1920*1080*3/2/4 = 777_600.
    assert_eq!(geom::out_buffer_size(1920, 1080), 777_600);
}

/// `set_restoration_unit_size` (`pcs.c:29`) — static, tier 4, and a constant.
///
/// Every parameter is `(void)`-cast away and the shift is a literal 0, so all
/// three planes take `RESTORATION_UNITSIZE_MAX`. Asserted so a future reader
/// does not have to re-derive that the dead parameters really are dead.
#[test]
fn traced_restoration_unit_size_is_constant() {
    assert_eq!(geom::restoration_unit_sizes(), [256, 256, 256]);
}

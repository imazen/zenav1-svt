use super::*;

/// C `svt_get_palette_cache_y` (palette.c:164-210): merge the above/left
/// neighbors' luma palettes into one sorted, deduped color cache for the
/// palette-color writer/cost fn. Above is DROPPED when `block_y` is at an
/// SB (64px) row top (C: `row % (1 << MIN_SB_SIZE_LOG2)` via
/// `-xd->mb_to_top_edge`, `MIN_SB_SIZE_LOG2 == 6`) — a rule specific to
/// this cache, NOT to [`EntropyCtx::palette_neighbor_ctx`]'s flag context.
/// Ties in the merge advance both cursors, keeping the ABOVE value (C's
/// `else` branch runs first and additionally drains `left` on equality).
// Consumed on BOTH sides now (#71, 2026-07-18): the MD `evaluate_leaf`
// reads this cache (via `commit_leaf`'s per-block `record_palette` stamp,
// coding order) into `search_palette_luma` + the cache-aware colour cost,
// and the PACK walk reads it for the palette-colour writer. On
// screen-content frames (EPICA) `above_palette`/`left_palette` DO carry
// nonzero sizes and the merge loop runs; on non-sc content no leaf wins a
// palette so it stays on the empty-cache early return (`above_n == 0 &&
// left_n == 0`), keeping those gates byte-identical.
pub(crate) fn palette_cache(
    ectx: &EntropyCtx,
    block_x: usize,
    block_y: usize,
) -> alloc::vec::Vec<u16> {
    let x4 = block_x / 4;
    let y4 = block_y / 4;
    let mut above_n = if !block_y.is_multiple_of(64) && x4 < ectx.above_palette.len() {
        ectx.above_palette[x4] as usize
    } else {
        0
    };
    let mut left_n = if y4 < ectx.left_palette.len() {
        ectx.left_palette[y4] as usize
    } else {
        0
    };
    if above_n == 0 && left_n == 0 {
        return alloc::vec::Vec::new();
    }
    let above_colors: &[u16] = if above_n > 0 {
        &ectx.above_palette_colors[x4][..above_n]
    } else {
        &[]
    };
    let left_colors: &[u16] = if left_n > 0 {
        &ectx.left_palette_colors[y4][..left_n]
    } else {
        &[]
    };
    let mut cache = alloc::vec::Vec::with_capacity(above_n + left_n);
    fn add(cache: &mut alloc::vec::Vec<u16>, v: u16) {
        // palette_add_to_cache (palette.c:154-161): skip a value equal to
        // the LAST entry already in the (ascending) cache.
        if cache.last() == Some(&v) {
            return;
        }
        cache.push(v);
    }
    let (mut ai, mut li) = (0usize, 0usize);
    while above_n > 0 && left_n > 0 {
        let v_above = above_colors[ai];
        let v_left = left_colors[li];
        if v_left < v_above {
            add(&mut cache, v_left);
            li += 1;
            left_n -= 1;
        } else {
            add(&mut cache, v_above);
            ai += 1;
            above_n -= 1;
            if v_left == v_above {
                li += 1;
                left_n -= 1;
            }
        }
    }
    while above_n > 0 {
        add(&mut cache, above_colors[ai]);
        ai += 1;
        above_n -= 1;
    }
    while left_n > 0 {
        add(&mut cache, left_colors[li]);
        li += 1;
        left_n -= 1;
    }
    debug_assert!(cache.len() <= 2 * svtav1_types::prediction::PALETTE_MAX_SIZE);
    cache
}

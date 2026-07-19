//! Palette pipeline (task #71) — C reference: `Source/Lib/Codec/palette.c`,
//! `k_means_template.h`, `random.h`, `pic_analysis_process.c`; the bd8 4:2:0
//! luma-only palette path (chroma palette is dead in this encoder:
//! `palette_size[1]` is hard-0 at injection, see the port map header).
//!
//! Landed here: chunk 1 (pure-math primitives), chunk 2 (color-map context /
//! tokenization), and chunk 3 ([`search_palette_luma`], dominant + k-means
//! search with the neighbour-cache centroid snap). RD integration + PACK +
//! MD-time state wiring live in `leaf_funnel.rs` / `pipeline.rs` and landed
//! with #71 injection. The remaining #71 work is calibration (screen-content
//! over-picking), not new palette primitives here.
//!
//! FFI-parity coverage (`tests/c_parity_palette.rs`): [`count_colors`],
//! [`index_color_cache`], [`k_means_dim1`] / [`calc_indices_dim1`] are
//! checked against the real C `svt_av1_count_colors` /
//! `svt_av1_index_color_cache` / `svt_av1_k_means_dim1_c` /
//! `svt_av1_calc_indices_dim1_c`. Everything reachable only through
//! `static` C functions ([`remove_duplicates`], [`optimize_palette_colors`],
//! [`extend_palette_color_map`], [`delta_encode_bits`],
//! [`palette_color_index_context`]) is instead validated by hand-derived
//! vectors traced against the C source, and carries a
//! `PORT-NOTE(unverified)` marker — see the CLAUDE.md BULK-PORT MODE index.

use svtav1_types::prediction::PALETTE_MAX_SIZE;

/// C `PALETTE_MIN_SIZE` (definitions.h:379).
pub const PALETTE_MIN_SIZE: usize = 2;

// =============================================================================
// Chunk 1: pure-math primitives
// =============================================================================

/// C `svt_av1_count_colors` (pic_analysis_process.c:892-909): builds a
/// 256-bin histogram of 8-bit luma values over the `rows`x`cols` window
/// (row stride `stride`) and returns the number of nonzero bins (distinct
/// colors). The histogram is written to `val_count` — C exposes it as an
/// out-parameter too (`int* val_count`) because the chunk-3 dominant-color
/// search reuses the same per-value counts (`search_palette_luma`,
/// palette.c:405-412).
pub fn count_colors(src: &[u8], stride: usize, rows: usize, cols: usize, val_count: &mut [i32; 256]) -> u16 {
    assert!(src.len() >= (rows - 1) * stride + cols);
    val_count.fill(0);
    for r in 0..rows {
        for c in 0..cols {
            val_count[src[r * stride + c] as usize] += 1;
        }
    }
    val_count.iter().filter(|&&n| n != 0).count() as u16
}

/// C `svt_av1_index_color_cache` (palette.c:111-141): splits `colors` into
/// those already present in the above/left `cache` (flagged in `found`,
/// one bool per cache entry) and those that are not (packed into the front
/// of `out`, preserving `colors`' original order). Returns the count of
/// not-found colors written to `out`.
///
/// `found` must hold at least `cache.len()` entries and `out` at least
/// `colors.len()`; `colors.len()` must not exceed [`PALETTE_MAX_SIZE`] (the
/// same fixed bound C's `in_cache_flags[PALETTE_MAX_SIZE]` scratch
/// assumes).
pub fn index_color_cache(cache: &[u16], colors: &[u16], found: &mut [bool], out: &mut [u16]) -> usize {
    assert!(found.len() >= cache.len());
    assert!(out.len() >= colors.len());
    let n_cache = cache.len();
    let n_colors = colors.len();
    if n_cache == 0 {
        out[..n_colors].copy_from_slice(colors);
        return n_colors;
    }
    assert!(colors.len() <= PALETTE_MAX_SIZE);
    found[..n_cache].fill(false);
    let mut in_cache_flags = [false; PALETTE_MAX_SIZE];
    let mut n_in_cache = 0usize;
    for i in 0..n_cache {
        if n_in_cache >= n_colors {
            break;
        }
        for j in 0..n_colors {
            if colors[j] == cache[i] {
                in_cache_flags[j] = true;
                found[i] = true;
                n_in_cache += 1;
                break;
            }
        }
    }
    let mut j = 0usize;
    for (i, &color) in colors.iter().enumerate() {
        if !in_cache_flags[i] {
            out[j] = color;
            j += 1;
        }
    }
    debug_assert_eq!(j, n_colors - n_in_cache);
    j
}

/// One value emitted as a fixed-width literal while delta-encoding
/// palette colors — either the first color (a `bit_depth`-bit literal) or
/// a later `(delta - min_val)` at its monotone-shrinking width, in
/// emission order. Shared by [`delta_encode_bits`] (the RD cost estimate)
/// and the future palette-color writer (`delta_encode_palette_colors`,
/// entropy_coding.c:4244-4276), which is line-for-line identical to C's
/// `delta_encode_cost` (palette.c:80-109) except it calls
/// `aom_write_literal(w, value, bits)` per step instead of summing `bits`
/// — exposing the shared step sequence here means the writer can never
/// derive a different bit-width sequence than the cost estimate did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeltaEncodeStep {
    /// The literal value to write (already offset by `min_val` for delta
    /// steps; the raw color for the first-literal step).
    pub value: u32,
    /// Width in bits this value is written with.
    pub bits: u32,
}

/// C `av1_ceil_log2` (cabac_context_model.h:366-371): `0` if `n < 2`,
/// else `floor(log2(n - 1)) + 1` (`get_msb(n - 1) + 1`, `get_msb` = GCC
/// `31 - __builtin_clz`, definitions.h:593-596).
fn ceil_log2(n: i32) -> i32 {
    if n < 2 {
        return 0;
    }
    let m = (n - 1) as u32;
    31 - m.leading_zeros() as i32 + 1
}

/// Builds the emission-order step sequence for delta-encoding `colors`
/// (already sorted ascending), matching C `delta_encode_cost`
/// (palette.c:80-109) exactly:
/// 1. `colors[0]` as a `bit_depth`-bit literal.
/// 2. (only if `colors.len() > 1`) the "bits-per-delta minus min_bits"
///    2-bit indicator, `min_bits = bit_depth - 3`.
/// 3. one `(colors[i] - colors[i-1] - min_val)` step per `i` in
///    `1..colors.len()`, each at a width that starts at
///    `max(av1_ceil_log2(max_delta + 1 - min_val), min_bits)` and can only
///    shrink (`AOMMIN` against `av1_ceil_log2(range)`) as the remaining
///    representable `range` (headroom from `colors[0]` to `1 <<
///    bit_depth`) shrinks after each step.
// PORT-NOTE(unverified): `delta_encode_cost` (palette.c:80-109) is `static`
// — no exported C symbol to FFI-test against. Verified only by hand-tracing
// the C source (this comment) and self-consistency in
// tests/c_parity_palette.rs. Upgrade path: add a ref_shims.c wrapper
// exposing it (or its `delta_encode_palette_colors` twin) once chunk 5
// lands, matching the existing shim pattern for other static C helpers.
pub fn delta_encode_steps(colors: &[u16], bit_depth: u32, min_val: u32) -> alloc::vec::Vec<DeltaEncodeStep> {
    let mut steps = alloc::vec::Vec::new();
    let num = colors.len();
    if num == 0 {
        return steps;
    }
    steps.push(DeltaEncodeStep {
        value: colors[0] as u32,
        bits: bit_depth,
    });
    if num == 1 {
        return steps;
    }
    let min_val = min_val as i32;
    let min_bits = bit_depth as i32 - 3;
    let mut max_delta = 0i32;
    let mut deltas = alloc::vec::Vec::with_capacity(num - 1);
    for i in 1..num {
        let delta = colors[i] as i32 - colors[i - 1] as i32;
        debug_assert!(delta >= min_val, "colors must be ascending with gaps >= min_val");
        deltas.push(delta);
        max_delta = max_delta.max(delta);
    }
    let mut bits_per_delta = ceil_log2(max_delta + 1 - min_val).max(min_bits);
    debug_assert!(bits_per_delta <= bit_depth as i32);
    steps.push(DeltaEncodeStep {
        value: (bits_per_delta - min_bits) as u32,
        bits: 2,
    });
    let mut range = (1i32 << bit_depth) - colors[0] as i32 - min_val;
    for delta in deltas {
        steps.push(DeltaEncodeStep {
            value: (delta - min_val) as u32,
            bits: bits_per_delta as u32,
        });
        range -= delta;
        bits_per_delta = bits_per_delta.min(ceil_log2(range));
    }
    steps
}

/// C `delta_encode_cost` (palette.c:80-109): total bit count to
/// delta-encode `colors` (ascending, e.g. the out-of-cache palette
/// colors from [`index_color_cache`]), reusing [`delta_encode_steps`] so
/// the estimate can never drift from the future writer.
// PORT-NOTE(unverified): see delta_encode_steps above — same static-C
// caveat applies (this is a thin sum over the same steps).
pub fn delta_encode_bits(colors: &[u16], bit_depth: u32, min_val: u32) -> u32 {
    delta_encode_steps(colors, bit_depth, min_val)
        .iter()
        .map(|s| s.bits)
        .sum()
}

/// C `DIVIDE_AND_ROUND` (utility.h:96): `(x + (y >> 1)) / y`, round-half-up
/// for the non-negative sums/counts k-means uses it on.
fn divide_and_round(x: i32, y: i32) -> i32 {
    (x + (y >> 1)) / y
}

/// C `lcg_next` (random.h:23-26): `*state = (uint32_t)(*state *
/// 1103515245ULL + 12345)`, returning the new state. The multiply/add
/// happen in 64-bit (matching the `ULL` literal's promotion) then
/// truncate back to 32 bits.
fn lcg_next(state: &mut u32) -> u32 {
    let next = (*state as u64) * 1103515245 + 12345;
    *state = next as u32;
    *state
}

/// C `lcg_rand16` (random.h:29-31): a value in `[0, 32768)`.
fn lcg_rand16(state: &mut u32) -> u32 {
    (lcg_next(state) / 65536) % 32768
}

/// C `svt_av1_calc_indices_dim1_c` (k_means_template.h:33-45, `dim=1`
/// instantiation via palette.c:55-56) fused with its immediate-successor
/// call `calc_total_dist` (k_means_template.h:76-84): every C call site
/// invokes `calc_indices` then `calc_total_dist` back to back on the same
/// `(data, centroids, indices, n, k)`, and the winning `min_dist` computed
/// per point while assigning `indices[i]` is exactly the term
/// `calc_total_dist` sums — so returning the running total here loses no
/// precision and can never diverge from calling both C functions in
/// sequence. Ties resolve to the FIRST (lowest) centroid index (strict
/// `<` comparison, matching C).
pub fn calc_indices_dim1(data: &[i32], centroids: &[i32], indices: &mut [u8], n: usize, k: usize) -> i64 {
    assert!(data.len() >= n);
    assert!(centroids.len() >= k);
    assert!(indices.len() >= n);
    assert!(k >= 1);
    let mut total: i64 = 0;
    for i in 0..n {
        let x = data[i];
        let mut min_dist = {
            let d = x - centroids[0];
            d * d
        };
        let mut idx = 0u8;
        for j in 1..k {
            let d = x - centroids[j];
            let this_dist = d * d;
            if this_dist < min_dist {
                min_dist = this_dist;
                idx = j as u8;
            }
        }
        indices[i] = idx;
        total += min_dist as i64;
    }
    total
}

/// C `calc_centroids` (k_means_template.h:47-74, `dim=1` instantiation):
/// recomputes each centroid as the rounded mean of its assigned points
/// (`DIVIDE_AND_ROUND`); an empty cluster is reseeded from a random data
/// point via `lcg_rand16`, seeded ONCE per call from `data[0]` (NOT
/// persisted across [`k_means_dim1`] iterations — a fresh `rand_state`
/// local every call, matching C's per-call `unsigned int rand_state =
/// (unsigned int)data[0]`). Multiple empty clusters in the same call draw
/// successive values from that one LCG stream.
fn calc_centroids_dim1(data: &[i32], centroids: &mut [i32; PALETTE_MAX_SIZE], indices: &[u8], n: usize, k: usize) {
    debug_assert!(n <= 32768);
    let mut count = [0i32; PALETTE_MAX_SIZE];
    let mut rand_state = data[0] as u32;
    for c in centroids.iter_mut().take(k) {
        *c = 0;
    }
    for i in 0..n {
        let index = indices[i] as usize;
        count[index] += 1;
        centroids[index] += data[i];
    }
    for i in 0..k {
        if count[i] == 0 {
            let pick = (lcg_rand16(&mut rand_state) as usize) % n;
            centroids[i] = data[pick];
        } else {
            centroids[i] = divide_and_round(centroids[i], count[i]);
        }
    }
}

/// C `svt_av1_k_means_dim1_c` (k_means_template.h:86-111, `dim=1`
/// instantiation via palette.c:55-56): iterative refinement starting from
/// caller-seeded `centroids`. Each iteration snapshots the current
/// (centroids, indices), recomputes centroids from the current
/// assignment, reassigns indices, and either: restores the snapshot and
/// stops if the new total distortion is WORSE than before (regression
/// rule), stops without restoring if centroids didn't change (converged),
/// or continues otherwise. `centroids` beyond `[..k]` and `indices`
/// beyond `[..n]` are left untouched, matching C's fixed-size scratch
/// that only ever addresses `k`/`n` elements.
pub fn k_means_dim1(
    data: &[i32],
    centroids: &mut [i32; PALETTE_MAX_SIZE],
    indices: &mut [u8],
    n: usize,
    k: usize,
    max_itr: u32,
) {
    assert!(data.len() >= n);
    assert!(indices.len() >= n);
    assert!(k >= 1 && k <= PALETTE_MAX_SIZE);

    let mut this_dist = calc_indices_dim1(data, centroids, indices, n, k);
    let mut pre_centroids = [0i32; PALETTE_MAX_SIZE];
    let mut pre_indices: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(n);

    for _ in 0..max_itr {
        let pre_dist = this_dist;
        pre_centroids[..k].copy_from_slice(&centroids[..k]);
        pre_indices.clear();
        pre_indices.extend_from_slice(&indices[..n]);

        calc_centroids_dim1(data, centroids, indices, n, k);
        this_dist = calc_indices_dim1(data, centroids, indices, n, k);

        if this_dist > pre_dist {
            centroids[..k].copy_from_slice(&pre_centroids[..k]);
            indices[..n].copy_from_slice(&pre_indices);
            break;
        }
        if centroids[..k] == pre_centroids[..k] {
            break;
        }
    }
}

/// C `av1_remove_duplicates` (palette.c:66-78): sorts `centroids[..k]`
/// ascending (`qsort` + a plain integer comparator — a Rust sort has no
/// stability concerns here since duplicate VALUES are indistinguishable)
/// then squeezes to the unique subsequence in place. Returns the new
/// count (bug-for-bug faithful: `k == 0` still returns `1`, matching C's
/// unconditional `num_unique = 1` — never exercised in practice since
/// real callers always pass `k >= PALETTE_MIN_SIZE`).
// PORT-NOTE(unverified): `av1_remove_duplicates` is `static` in palette.c
// — no exported C symbol. Verified only by hand-tracing the C source; see
// tests/c_parity_palette.rs for constructed-input checks against manually
// computed expected output.
pub fn remove_duplicates(centroids: &mut [i32], k: usize) -> usize {
    centroids[..k].sort_unstable();
    let mut num_unique = 1usize;
    for i in 1..k {
        if centroids[i] != centroids[i - 1] {
            centroids[num_unique] = centroids[i];
            num_unique += 1;
        }
    }
    num_unique
}

/// C `optimize_palette_colors` (palette.c:250-270), specialized to
/// `stride == 1` — the only value ever passed
/// (`palette_rd_y`, palette.c:300); the dim-2/chroma `stride == 2` case is
/// dead in this port (`k_means_dim2` has zero callers — see the port map
/// header). Biases centroids toward nearby colors already in the
/// above/left cache: each centroid within `(6 + (qp_index >> 6)) <<
/// (bit_depth - 8)` of its nearest cache color snaps to that color (ties
/// -> lowest cache index, matching the strict `<` C comparison).
// PORT-NOTE(unverified): `optimize_palette_colors` is `static
// AOM_INLINE` in palette.c — no exported C symbol. Verified only by
// hand-tracing the C source; see tests/c_parity_palette.rs.
pub fn optimize_palette_colors(color_cache: &[u16], n_cache: usize, centroids: &mut [i32], k: usize, qp_index: u8, bit_depth: u32) {
    if n_cache == 0 {
        return;
    }
    let min_threshold = (6i32 + (qp_index as i32 >> 6)) << (bit_depth - 8);
    for i in 0..k {
        let mut min_diff = (centroids[i] - color_cache[0] as i32).abs();
        let mut idx = 0usize;
        for j in 1..n_cache {
            let this_diff = (centroids[i] - color_cache[j] as i32).abs();
            if this_diff < min_diff {
                min_diff = this_diff;
                idx = j;
            }
        }
        if min_diff <= min_threshold {
            centroids[i] = color_cache[idx] as i32;
        }
    }
}

/// C `extend_palette_color_map` (palette.c:275-294). `color_map` holds an
/// `orig_height x orig_width` map at `orig_width` stride in its first
/// `orig_width*orig_height` bytes; grows it IN PLACE to `new_height x
/// new_width` at `new_width` stride by replicating the last column of
/// each row, then replicating the last row. Rows are processed in
/// DECREASING order for the column-extension pass — the destination
/// stride is wider than the source, so forward order would overwrite
/// not-yet-moved source rows before they are read.
// PORT-NOTE(unverified): `extend_palette_color_map` is `static
// AOM_INLINE` in palette.c — no exported C symbol. Verified only by
// hand-tracing the C source; see tests/c_parity_palette.rs.
pub fn extend_palette_color_map(
    color_map: &mut [u8],
    orig_width: usize,
    orig_height: usize,
    new_width: usize,
    new_height: usize,
) {
    assert!(new_width >= orig_width);
    assert!(new_height >= orig_height);
    if new_width == orig_width && new_height == orig_height {
        return;
    }
    for j in (0..orig_height).rev() {
        color_map.copy_within(j * orig_width..j * orig_width + orig_width, j * new_width);
        if new_width > orig_width {
            let fill = color_map[j * new_width + orig_width - 1];
            color_map[j * new_width + orig_width..j * new_width + new_width].fill(fill);
        }
    }
    for j in orig_height..new_height {
        color_map.copy_within((orig_height - 1) * new_width..orig_height * new_width, j * new_width);
    }
}

// =============================================================================
// Chunk 2: color-map context / tokenization skeleton
// =============================================================================

/// C `svt_aom_palette_color_index_context_lookup` (palette.c:608):
/// `ctx = LOOKUP[hash]`. Every reachable hash (2, 5, 6, 7, or 8 — see
/// tests) maps to an entry equal to `9 - hash`; the other entries are `-1`
/// ("negative values are invalid", per the C comment) and never produced
/// by [`palette_color_index_context`]. Kept only to self-check that
/// equivalence, mirroring the C `assert`.
const PALETTE_COLOR_INDEX_CONTEXT_LOOKUP: [i32; 9] = [-1, -1, 0, -1, -1, 4, 3, 2, 1];

const INVALID_COLOR_IDX: u8 = u8::MAX;

/// C `av1_fast_palette_color_index_context` + `_on_edge`
/// (palette.c:612-743): the rank-remapped color index and entropy context
/// for one palette-map pixel `(i, j)` (row, col) during the wavefront
/// pass — never called for `(0, 0)`, which is coded with `write_uniform`
/// instead (see [`color_map_wavefront`]). `palette_size` is used only to
/// assert the RANK-REMAPPED `color_new_idx` invariant `< palette_size`
/// (the C call site's assert, `cost_and_tokenize_map`, palette.c:768).
///
/// Returns `(ctx, color_new_idx)`.
// PORT-NOTE(unverified): both C functions are `static inline` in
// palette.c — no exported symbol, so this cannot be FFI-differential-
// tested directly. Validated instead by: (1) an exhaustive check that
// every valid PALETTE_COLOR_INDEX_CONTEXT_LOOKUP entry equals `9 - hash`
// (tests/c_parity_palette.rs), and (2) hand-derived vectors traced
// step-by-step against this C source across all 5 reachable contexts (0
// via both edge orientations, 1-4 via the interior merge/sort branches),
// documented in the same test file. Upgrade path: add a ref_shims.c
// wrapper exposing these two static functions directly, once justified by
// a chunk-3+ need.
pub fn palette_color_index_context(color_map: &[u8], stride: usize, i: usize, j: usize, palette_size: usize) -> (usize, u8) {
    assert!(i > 0 || j > 0);
    let has_above = i >= 1;
    let has_left = j >= 1;
    assert!(has_above || has_left);

    let (ctx, color_new_idx) = if has_above != has_left {
        // Edge case: exactly one neighbor (top row or left column).
        let neighbor = if has_above {
            color_map[(i - 1) * stride + j]
        } else {
            color_map[i * stride + (j - 1)]
        };
        let current = color_map[i * stride + j];
        let idx = if neighbor > current {
            current + 1
        } else if neighbor == current {
            0
        } else {
            current
        };
        // color_score=2, hash_multiplier=1 => hash=2 => lookup[2]=0.
        debug_assert_eq!(PALETTE_COLOR_INDEX_CONTEXT_LOOKUP[2], 0);
        (0usize, idx)
    } else {
        // Interior case: three neighbors (left, top, top-left).
        let mut color_neighbors = [
            color_map[i * stride + (j - 1)],
            color_map[(i - 1) * stride + j],
            color_map[(i - 1) * stride + (j - 1)],
        ];
        let mut scores = [2u8, 2u8, 1u8];
        let mut num_invalid_colors = 0u8;
        if color_neighbors[0] == color_neighbors[1] {
            scores[0] += scores[1];
            color_neighbors[1] = INVALID_COLOR_IDX;
            num_invalid_colors += 1;
            if color_neighbors[0] == color_neighbors[2] {
                scores[0] += scores[2];
                num_invalid_colors += 1;
            }
        } else if color_neighbors[0] == color_neighbors[2] {
            scores[0] += scores[2];
            num_invalid_colors += 1;
        } else if color_neighbors[1] == color_neighbors[2] {
            scores[1] += scores[2];
            num_invalid_colors += 1;
        }
        let num_valid_colors = 3 - num_invalid_colors;

        if num_valid_colors > 1 {
            if color_neighbors[1] == INVALID_COLOR_IDX {
                scores[1] = scores[2];
                color_neighbors[1] = color_neighbors[2];
            }
            if scores[0] < scores[1] || (scores[0] == scores[1] && color_neighbors[0] > color_neighbors[1]) {
                scores.swap(0, 1);
                color_neighbors.swap(0, 1);
            }
            if num_valid_colors > 2 {
                if scores[0] < scores[2] {
                    scores.swap(0, 2);
                    color_neighbors.swap(0, 2);
                }
                if scores[1] < scores[2] {
                    scores.swap(1, 2);
                    color_neighbors.swap(1, 2);
                }
            }
        }

        let current = color_map[i * stride + j];
        let mut color_new_idx = current;
        for idx in 0..num_valid_colors as usize {
            if color_neighbors[idx] > current {
                color_new_idx += 1;
            } else if color_neighbors[idx] == current {
                color_new_idx = idx as u8;
                break;
            }
        }

        const HASH_MULTIPLIERS: [u8; 3] = [1, 2, 2];
        let mut hash = 0u8;
        for idx in 0..num_valid_colors as usize {
            hash += scores[idx] * HASH_MULTIPLIERS[idx];
        }
        debug_assert!(hash > 0 && hash <= 8);
        let ctx = 9 - hash as i32;
        debug_assert_eq!(ctx, PALETTE_COLOR_INDEX_CONTEXT_LOOKUP[hash as usize]);
        (ctx as usize, color_new_idx)
    };

    debug_assert!(ctx < 5, "PALETTE_COLOR_INDEX_CONTEXTS == 5");
    assert!((color_new_idx as usize) < palette_size);
    (ctx, color_new_idx)
}

/// Anti-diagonal ("wavefront") traversal of a palette color map, EXCLUDING
/// `(0, 0)` (coded separately with `write_uniform`) — the shared
/// iteration order for both the palette-map cost estimate
/// (`svt_av1_cost_color_map`) and the tokenizer
/// (`svt_av1_tokenize_color_map`), C `cost_and_tokenize_map`
/// (palette.c:748-782, loop structure only — `k` in `1..=rows+cols-2`,
/// `j` from `min(k, cols-1)` DOWN TO `max(0, k-rows+1)`, `i = k - j`).
/// Calls `f(i, j, ctx, color_new_idx)` for every pixel in decode order
/// (required because each pixel's context depends on already-visited
/// neighbors).
// PORT-NOTE(unverified): the loop this wraps lives inside the `static`
// `cost_and_tokenize_map` (palette.c:748-782) — no exported symbol.
// Verified by hand-derivation of the traversal order for several
// (rows, cols) shapes in tests/c_parity_palette.rs (cross-checked for
// correct anti-diagonal dependency ordering), not a C differential.
pub fn color_map_wavefront<F: FnMut(usize, usize, usize, u8)>(
    color_map: &[u8],
    stride: usize,
    rows: usize,
    cols: usize,
    palette_size: usize,
    mut f: F,
) {
    debug_assert!(rows >= 1 && cols >= 1);
    for k in 1..(rows + cols - 1) {
        let j_hi = k.min(cols - 1);
        let j_lo = k.saturating_sub(rows - 1);
        for j in (j_lo..=j_hi).rev() {
            let i = k - j;
            let (ctx, color_new_idx) = palette_color_index_context(color_map, stride, i, j, palette_size);
            f(i, j, ctx, color_new_idx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ceil_log2_matches_c_definition() {
        // av1_ceil_log2: 0 if n<2, else floor(log2(n-1))+1.
        assert_eq!(ceil_log2(0), 0);
        assert_eq!(ceil_log2(1), 0);
        assert_eq!(ceil_log2(2), 1); // floor(log2(1))+1 = 0+1
        assert_eq!(ceil_log2(3), 2); // floor(log2(2))+1 = 1+1
        assert_eq!(ceil_log2(4), 2); // floor(log2(3))+1 = 1+1
        assert_eq!(ceil_log2(5), 3); // floor(log2(4))+1 = 2+1
        assert_eq!(ceil_log2(9), 4); // floor(log2(8))+1 = 3+1
        assert_eq!(ceil_log2(256), 8); // floor(log2(255))+1 = 7+1
        assert_eq!(ceil_log2(257), 9); // floor(log2(256))+1 = 8+1
    }

    #[test]
    fn divide_and_round_half_up() {
        assert_eq!(divide_and_round(10, 4), 3); // (10+2)/4 = 3
        assert_eq!(divide_and_round(9, 4), 2); // (9+2)/4 = 2 (2.75 truncates)
        assert_eq!(divide_and_round(0, 5), 0);
        assert_eq!(divide_and_round(7, 1), 7);
    }

    #[test]
    fn lcg_matches_c_first_values() {
        // First two outputs of lcg_next/lcg_rand16 seeded from 0, hand
        // computed from random.h's formula: state = state*1103515245+12345.
        let mut state = 0u32;
        let n1 = lcg_next(&mut state);
        assert_eq!(n1, 12345);
        assert_eq!(state, 12345);
        let r1 = (n1 / 65536) % 32768;
        assert_eq!(r1, 0);
        let mut state2 = 0u32;
        let v1 = lcg_rand16(&mut state2);
        assert_eq!(v1, r1);
        let v2 = lcg_rand16(&mut state2);
        // second raw state = 12345*1103515245+12345 mod 2^32
        let expect_state2 = (12345u64 * 1103515245 + 12345) as u32;
        assert_eq!(state2, expect_state2);
        assert_eq!(v2, (expect_state2 / 65536) % 32768);
    }
}

// ============================================================================
// Chunk 3: per-block palette search (search_palette_luma + palette_rd_y,
// palette.c:296-530) — bd8 path only (hbd_md == 0 on this port's target;
// see docs/palette-port-map.md).
// ============================================================================

/// Per-level palette search knobs — C `set_palette_level`
/// (enc_mode_config.c:1841-1915). Allintra-reachable levels are
/// {0, 2, 3, 4, 5, 7} (sig_deriv_multi_processes_allintra :2374-2390);
/// `centroid_refinement` is 0 for all of them, so
/// `cache_based_centroid_refinement` is NOT ported (dead on this path).
/// 0xFF = arm disabled, mirroring C's `(uint8_t)~0` sentinel.
#[derive(Clone, Copy, Debug, Default)]
pub struct PaletteCtrls {
    pub enabled: bool,
    pub dominant_color_step: u8,
    pub kmean_color_step: u8,
    pub k_means_max_itr: u32,
}

impl PaletteCtrls {
    /// C `set_palette_level` rows for the allintra-reachable levels.
    pub fn for_level(level: u8) -> Self {
        match level {
            0 => PaletteCtrls::default(),
            2 => PaletteCtrls { enabled: true, dominant_color_step: 2, kmean_color_step: 1, k_means_max_itr: 2 },
            3 => PaletteCtrls { enabled: true, dominant_color_step: 0xFF, kmean_color_step: 1, k_means_max_itr: 2 },
            4 => PaletteCtrls { enabled: true, dominant_color_step: 0xFF, kmean_color_step: 2, k_means_max_itr: 2 },
            5 => PaletteCtrls { enabled: true, dominant_color_step: 0xFF, kmean_color_step: 3, k_means_max_itr: 2 },
            7 => PaletteCtrls { enabled: true, dominant_color_step: 0xFF, kmean_color_step: 5, k_means_max_itr: 1 },
            // PORT-NOTE(unverified): levels 1/6/8/9 exist in C but are
            // unreachable from the allintra derivation; transcribe if a
            // non-allintra mode ever needs them.
            _ => PaletteCtrls::default(),
        }
    }
}

/// One produced palette candidate: the deduped ascending colors and the
/// full nominal-size (block_w x block_h) color index map.
#[derive(Clone, Debug)]
pub struct PaletteCand {
    pub colors: alloc::vec::Vec<u16>,
    pub idx_map: alloc::vec::Vec<u8>,
}

/// C `palette_rd_y` (palette.c:296-325) minus the ctx plumbing: refine +
/// dedup the centroids, reject k < 2, then recompute the AUTHORITATIVE map
/// against the final sorted list and extend it to nominal dims. Returns
/// None on rejection (C leaves palette_size_array[0] = 0 and the caller
/// reuses the slot).
#[allow(clippy::too_many_arguments)]
fn palette_rd_y(
    data: &[i32],
    centroids: &mut [i32],
    n: usize,
    opt_colors: bool,
    color_cache: &[u16],
    qp_index: u8,
    rows: usize,
    cols: usize,
    block_w: usize,
    block_h: usize,
) -> Option<PaletteCand> {
    if opt_colors {
        optimize_palette_colors(color_cache, color_cache.len(), centroids, n, qp_index, 8);
    }
    let k = remove_duplicates(centroids, n);
    if k < PALETTE_MIN_SIZE {
        return None;
    }
    // bd8: clip_pixel (0..=255).
    let colors: alloc::vec::Vec<u16> = centroids[..k]
        .iter()
        .map(|&c| c.clamp(0, 255) as u16)
        .collect();
    let mut idx_map = alloc::vec![0u8; block_w * block_h];
    calc_indices_dim1(data, &centroids[..k], &mut idx_map, rows * cols, k);
    extend_palette_color_map(&mut idx_map, cols, rows, block_w, block_h);
    Some(PaletteCand { colors, idx_map })
}

/// C `search_palette_luma` (palette.c:388-530), bd8. `src` is the SOURCE
/// luma plane (C enhanced_pic); `(abs_x, abs_y)` the block origin;
/// `(rows, cols)` the within-bounds dims and `(block_w, block_h)` the
/// nominal dims (C svt_aom_get_block_dimensions — equal except at
/// non-aligned right/bottom picture edges). `cache` is the neighbor color
/// cache (svt_get_palette_cache_y — chunk 6 wires the real neighbor
/// state; empty slice = no neighbors, bit-exact for isolated blocks).
///
/// Appends up to 14 candidates; rejected sizes reuse their slot exactly
/// like C's `(*tot_palette_cands)++` gating.
///
/// PORT-NOTE(unverified): RD integration (#71 injection) HAS landed, so this
/// runs end-to-end on the EPICA p6/p7 cells — but those cells do not
/// byte-match C yet (palette over-picking, #71), so it is exercised-but-not-
/// yet-byte-verified. The two transcription-fragile spots to re-check when a
/// cell diverges on palette COLORS: the dominant-color argmax tie (first-max
/// => LOWEST pixel value wins) and the integer seed expression
/// `lb + (2i+1)*(ub-lb)/n/2` (divide by n THEN by 2, not by 2n).
#[allow(clippy::too_many_arguments)]
pub fn search_palette_luma(
    src: &[u8],
    stride: usize,
    abs_x: usize,
    abs_y: usize,
    rows: usize,
    cols: usize,
    block_w: usize,
    block_h: usize,
    ctrls: &PaletteCtrls,
    cache: &[u16],
    qp_index: u8,
) -> alloc::vec::Vec<PaletteCand> {
    let mut out = alloc::vec::Vec::new();
    if !ctrls.enabled {
        return out;
    }
    let mut count_buf = [0i32; 256];
    let origin = abs_y * stride + abs_x;
    let colors = count_colors(&src[origin..], stride, rows, cols, &mut count_buf) as usize;
    if colors <= 1 || colors > 64 {
        return out;
    }
    let max_n = colors.min(svtav1_types::prediction::PALETTE_MAX_SIZE);
    let min_n = PALETTE_MIN_SIZE;

    // data[] + lb/ub (palette.c:421-439). C seeds lb=ub=src[0] BEFORE the
    // loop; the loop then min/maxes every pixel including [0] — plain
    // min/max over the block.
    let mut data = alloc::vec![0i32; rows * cols];
    let mut lb = i32::from(src[origin]);
    let mut ub = lb;
    for r in 0..rows {
        for c in 0..cols {
            let v = i32::from(src[origin + r * stride + c]);
            data[r * cols + c] = v;
            lb = lb.min(v);
            ub = ub.max(v);
        }
    }

    let mut centroids = [0i32; 8];

    // A) Dominant-color candidates (palette.c:440-478). The argmax scan is
    // strict `>` ascending over j => on tied counts the LOWEST pixel value
    // wins; each round zeroes the picked bin. NOTE: consumes count_buf.
    if ctrls.dominant_color_step != 0xFF {
        let mut top_colors = [0i32; 8];
        for i in 0..max_n {
            let mut max_count = 0i32;
            for (j, &cnt) in count_buf.iter().enumerate() {
                if cnt > max_count {
                    max_count = cnt;
                    top_colors[i] = j as i32;
                }
            }
            count_buf[top_colors[i] as usize] = 0;
        }
        let mut n = max_n as i32;
        while n >= min_n as i32 {
            centroids[..n as usize].copy_from_slice(&top_colors[..n as usize]);
            if let Some(cand) = palette_rd_y(
                &data, &mut centroids[..n as usize], n as usize,
                false, &[], qp_index, rows, cols, block_w, block_h,
            ) {
                out.push(cand);
            }
            n -= i32::from(ctrls.dominant_color_step);
        }
    }

    // B) K-means candidates (palette.c:480-529).
    if ctrls.kmean_color_step != 0xFF {
        let mut indices = alloc::vec![0u8; rows * cols];
        let mut n = max_n as i32;
        while n >= min_n as i32 {
            let nn = n as usize;
            if colors == PALETTE_MIN_SIZE {
                centroids[0] = lb;
                centroids[1] = ub;
            } else {
                for i in 0..nn {
                    // C: lb + (2*i+1)*(ub-lb)/n/2 — sequential integer
                    // divisions, NOT /(2n).
                    centroids[i] = lb + (2 * i as i32 + 1) * (ub - lb) / n / 2;
                }
                k_means_dim1(
                    &data,
                    &mut centroids,
                    &mut indices,
                    rows * cols,
                    nn,
                    ctrls.k_means_max_itr,
                );
            }
            // centroid_refinement: 0 at every allintra level — not ported.
            if let Some(cand) = palette_rd_y(
                &data, &mut centroids[..nn], nn,
                true, cache, qp_index, rows, cols, block_w, block_h,
            ) {
                out.push(cand);
            }
            n -= i32::from(ctrls.kmean_color_step);
        }
    }
    out
}

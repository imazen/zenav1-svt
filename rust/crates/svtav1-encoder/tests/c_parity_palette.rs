//! Differential parity: palette pipeline chunk 1 (pure-math primitives) and
//! chunk 2 (color-map context/tokenization skeleton) vs the real C reference
//! (task #71, `docs/palette-port-map.md`).
//!
//! FFI-differential (real C `svt_av1_count_colors` / `svt_av1_index_color_
//! cache` / `svt_av1_k_means_dim1_c` / `svt_av1_calc_indices_dim1_c`,
//! Source/Lib/Codec/palette.c + pic_analysis_process.c):
//! [count_colors_matches_c], [index_color_cache_matches_c],
//! [k_means_dim1_matches_c] (incl. n%16!=0 edge sizes and a scenario forcing
//! empty-cluster LCG reseeding), [calc_indices_dim1_matches_c].
//!
//! Everything else in this file validates code that is reachable only
//! through a `static` C function (no exported symbol to call): the tests
//! are hand-derived vectors traced step-by-step against the C source
//! (palette.c), documented inline at each assertion. This is this project's
//! WEAKEST evidence tier (verbatim transcription) — see the
//! `PORT-NOTE(unverified)` markers in `src/palette.rs`.

use svtav1_cref as cref;
use svtav1_encoder::palette;
use svtav1_types::prediction::PALETTE_MAX_SIZE;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn byte(&mut self) -> u8 {
        (self.next() >> 32) as u8
    }
}

// =============================================================================
// FFI-differential: chunk 1 primitives with exported C symbols.
// =============================================================================

#[test]
fn count_colors_matches_c() {
    let mut rng = Rng(0xc0_107_5eed);
    let shapes = [(8usize, 8usize, 8usize), (16, 16, 16), (8, 8, 23), (13, 17, 31), (64, 64, 64)];
    for &(rows, cols, stride) in &shapes {
        for ncolors in [1usize, 2, 5, 16, 64, 200] {
            let palette: Vec<u8> = (0..ncolors).map(|_| rng.byte()).collect();
            for _ in 0..10 {
                let mut buf = vec![0u8; rows * stride];
                for r in 0..rows {
                    for c in 0..cols {
                        buf[r * stride + c] = palette[rng.below(ncolors as u64) as usize];
                    }
                }
                let mut hist_r = [0i32; 256];
                let mut hist_c = [0i32; 256];
                let n_r = palette::count_colors(&buf, stride, rows, cols, &mut hist_r);
                let n_c = cref::count_colors(&buf, stride, rows, cols, &mut hist_c);
                assert_eq!(n_r as i32, n_c, "{rows}x{cols}s{stride} ncolors={ncolors}");
                assert_eq!(hist_r, hist_c, "{rows}x{cols}s{stride} ncolors={ncolors} histogram");
            }
        }
    }
}

/// `cache`/`colors` are generated sorted-ascending-unique, matching the real
/// invariant `svt_get_palette_cache_y`'s dedup-merge guarantees for `cache`
/// (and `remove_duplicates` guarantees for `colors`) — an unsorted or
/// duplicate-containing `cache` is out of the function's documented domain
/// (C's own `assert(j == n_colors - n_in_cache)` can fail for a cache with
/// duplicate values, since each duplicate re-matches and double-counts
/// `n_in_cache`).
fn unique_ascending(rng: &mut Rng, count: usize, max_val: u16) -> Vec<u16> {
    let mut set = Vec::new();
    while set.len() < count {
        let v = (rng.below(max_val as u64 + 1)) as u16;
        if !set.contains(&v) {
            set.push(v);
        }
    }
    set.sort_unstable();
    set
}

#[test]
fn index_color_cache_matches_c() {
    let mut rng = Rng(0x1_dea_c0de);
    for _trial in 0..400 {
        let n_cache = rng.below(17) as usize; // 0..=16 (2*PALETTE_MAX_SIZE)
        let n_colors = 1 + rng.below(PALETTE_MAX_SIZE as u64) as usize; // 1..=8
        let cache = unique_ascending(&mut rng, n_cache, 300);
        // Build colors with a mix of cache-hits and misses for real coverage.
        let mut colors = Vec::new();
        while colors.len() < n_colors {
            let v = if !cache.is_empty() && rng.below(2) == 0 {
                cache[rng.below(cache.len() as u64) as usize]
            } else {
                rng.below(301) as u16
            };
            if !colors.contains(&v) {
                colors.push(v);
            }
        }
        colors.sort_unstable();

        let mut found_r = vec![false; n_cache.max(1)];
        let mut out_r = vec![0u16; n_colors];
        let j_r = palette::index_color_cache(&cache, &colors, &mut found_r, &mut out_r);

        let mut found_c = vec![0u8; n_cache.max(1)];
        let mut out_c_i32 = vec![0i32; n_colors];
        let j_c = cref::index_color_cache(&cache, &colors, &mut found_c, &mut out_c_i32);

        assert_eq!(j_r as i32, j_c, "cache={cache:?} colors={colors:?}");
        assert_eq!(&out_r[..j_r], &out_c_i32[..j_r as usize].iter().map(|&v| v as u16).collect::<Vec<_>>(), "out mismatch cache={cache:?} colors={colors:?}");
        if n_cache > 0 {
            let found_r_bytes: Vec<u8> = found_r[..n_cache].iter().map(|&b| b as u8).collect();
            assert_eq!(found_r_bytes, found_c[..n_cache], "found mismatch cache={cache:?} colors={colors:?}");
        }
    }
}

/// Build a `[i32; PALETTE_MAX_SIZE]` centroid buffer from a slice, zero-padded.
fn centroid_buf(seed: &[i32]) -> [i32; PALETTE_MAX_SIZE] {
    let mut c = [0i32; PALETTE_MAX_SIZE];
    c[..seed.len()].copy_from_slice(seed);
    c
}

#[test]
fn calc_indices_dim1_matches_c() {
    let mut rng = Rng(0xca1c_1de5_5eed);
    // Deliberately include n % 16 != 0 (edge sizes for a future AVX2 cross-
    // check per the port map RISK section — this test only exercises `_c`).
    let ns = [1usize, 2, 5, 15, 16, 17, 33, 50, 63, 65, 100, 257, 4096];
    for &n in &ns {
        for k in [1usize, 2, 3, 5, 8] {
            for _ in 0..8 {
                let data: Vec<i32> = (0..n).map(|_| rng.below(4096) as i32).collect();
                let centroids: Vec<i32> = (0..k).map(|_| rng.below(4096) as i32).collect();

                let mut idx_r = vec![0u8; n];
                let total_r = palette::calc_indices_dim1(&data, &centroids, &mut idx_r, n, k);

                let mut idx_c = vec![0u8; n];
                cref::calc_indices_dim1(&data, &centroids, &mut idx_c, k);

                assert_eq!(idx_r, idx_c, "n={n} k={k} indices mismatch");

                // Self-check the fused total distance against a brute-force
                // recomputation (calc_total_dist has no exported C symbol).
                let brute: i64 = (0..n)
                    .map(|i| {
                        let d = data[i] - centroids[idx_r[i] as usize];
                        (d * d) as i64
                    })
                    .sum();
                assert_eq!(total_r, brute, "n={n} k={k} fused total_dist mismatch");
            }
        }
    }
}

#[test]
fn k_means_dim1_matches_c() {
    let mut rng = Rng(0xc0ffee_5eed);
    let ns = [2usize, 15, 16, 17, 50, 63, 65, 200, 4096];
    for &n in &ns {
        for k in [1usize, 2, 3, 5, 8] {
            for &max_itr in &[0u32, 1, 2, 50] {
                for _ in 0..6 {
                    let data: Vec<i32> = (0..n).map(|_| rng.below(256) as i32).collect();
                    let seed: Vec<i32> = (0..k).map(|_| rng.below(256) as i32).collect();

                    let mut cr = centroid_buf(&seed);
                    let mut idx_r = vec![0u8; n];
                    palette::k_means_dim1(&data, &mut cr, &mut idx_r, n, k, max_itr);

                    let mut cc = seed.clone();
                    let mut idx_c = vec![0u8; n];
                    cref::k_means_dim1(&data, &mut cc, &mut idx_c, k, max_itr as i32);

                    assert_eq!(&cr[..k], &cc[..k], "n={n} k={k} max_itr={max_itr} centroids");
                    assert_eq!(idx_r, idx_c, "n={n} k={k} max_itr={max_itr} indices");
                }
            }
        }
    }
}

/// Deliberately forces empty-cluster reassignment (far more centroids `k`
/// than distinct data values), exercising `calc_centroids_dim1`'s
/// `lcg_rand16` reseed path — a scenario general random data rarely
/// triggers since it usually spreads across enough distinct values that
/// every cluster gets at least one point.
#[test]
fn k_means_dim1_empty_cluster_reseed_matches_c() {
    let mut rng = Rng(0x5eed_c1a55);
    for _ in 0..30 {
        let n = 40usize;
        let k = 8usize; // only 2 distinct data values -> >=6 empty clusters
        let mut data = vec![5i32; 20];
        data.extend(vec![500i32; 20]);
        // Shuffle so cluster assignment isn't trivially position-correlated.
        for i in (1..n).rev() {
            let j = rng.below(i as u64 + 1) as usize;
            data.swap(i, j);
        }
        let seed: Vec<i32> = (0..k).map(|i| 5 + (2 * i as i32 + 1) * (500 - 5) / k as i32 / 2).collect();

        let mut cr = centroid_buf(&seed);
        let mut idx_r = vec![0u8; n];
        palette::k_means_dim1(&data, &mut cr, &mut idx_r, n, k, 2);

        let mut cc = seed.clone();
        let mut idx_c = vec![0u8; n];
        cref::k_means_dim1(&data, &mut cc, &mut idx_c, k, 2);

        assert_eq!(&cr[..k], &cc[..k], "reseed centroids data={data:?} seed={seed:?}");
        assert_eq!(idx_r, idx_c, "reseed indices data={data:?} seed={seed:?}");
    }
}

// =============================================================================
// Hand-derived vectors: functions behind `static` C symbols (chunk 1).
// =============================================================================

#[test]
fn remove_duplicates_hand_vectors() {
    // Sorted-ascending + unique-squeeze, traced from palette.c:66-78.
    let mut c = [5, 3, 3, 1, 9, 1, 0, 0];
    assert_eq!(palette::remove_duplicates(&mut c, 6), 4);
    assert_eq!(&c[..4], &[1, 3, 5, 9]);

    let mut c = [7, 7, 7, 0, 0, 0, 0, 0];
    assert_eq!(palette::remove_duplicates(&mut c, 3), 1);
    assert_eq!(c[0], 7);

    let mut c = [4, 2, 8, 0, 0, 0, 0, 0];
    assert_eq!(palette::remove_duplicates(&mut c, 3), 3);
    assert_eq!(&c[..3], &[2, 4, 8]);

    // k=1: loop body never runs, still returns 1.
    let mut c = [42, 0, 0, 0, 0, 0, 0, 0];
    assert_eq!(palette::remove_duplicates(&mut c, 1), 1);
    assert_eq!(c[0], 42);

    // k=0: C's `num_unique = 1` is unconditional -- bug-for-bug quirk, never
    // exercised by a real caller (always k >= PALETTE_MIN_SIZE).
    let mut c: [i32; 0] = [];
    assert_eq!(palette::remove_duplicates(&mut c, 0), 1);
}

#[test]
fn optimize_palette_colors_hand_vectors() {
    // qp_index=0, bit_depth=8 => min_threshold = (6+0)<<0 = 6.
    let cache = [10u16, 50, 100];
    let mut centroids = [12i32, 60, 200];
    palette::optimize_palette_colors(&cache, 3, &mut centroids, 3, 0, 8);
    // c0: nearest=10 (diff2<=6) -> snap. c1: nearest=50(diff10>6) -> unchanged.
    // c2: nearest=100(diff100>6) -> unchanged.
    assert_eq!(centroids, [10, 60, 200]);

    // qp_index=200 (>>6=3), bit_depth=10 => threshold=(6+3)<<2=36.
    let cache = [100u16, 500];
    let mut centroids = [130i32, 700];
    palette::optimize_palette_colors(&cache, 2, &mut centroids, 2, 200, 10);
    assert_eq!(centroids, [100, 700]);

    // n_cache=0 -> no-op.
    let cache: [u16; 0] = [];
    let mut centroids = [5i32, 10];
    palette::optimize_palette_colors(&cache, 0, &mut centroids, 2, 0, 8);
    assert_eq!(centroids, [5, 10]);

    // Tie (diff to both cache entries == 5): strict `<` keeps the FIRST
    // (lowest-index) cache entry, matching C.
    let cache = [10u16, 20];
    let mut centroids = [15i32];
    palette::optimize_palette_colors(&cache, 2, &mut centroids, 1, 0, 8);
    assert_eq!(centroids, [10]);
}

#[test]
fn extend_palette_color_map_hand_vectors() {
    // orig 3x2 (width x height) -> new 5x4: column replication then row replication.
    let mut buf = vec![0xAAu8; 20];
    buf[0..6].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    palette::extend_palette_color_map(&mut buf, 3, 2, 5, 4);
    #[rustfmt::skip]
    assert_eq!(buf, vec![
        1, 2, 3, 3, 3,
        4, 5, 6, 6, 6,
        4, 5, 6, 6, 6,
        4, 5, 6, 6, 6,
    ]);

    // Width-only extension (new_height == orig_height): no row-replication pass.
    let mut buf = vec![0xAAu8; 8];
    buf[0..4].copy_from_slice(&[1, 2, 3, 4]);
    palette::extend_palette_color_map(&mut buf, 2, 2, 4, 2);
    assert_eq!(buf, vec![1, 2, 2, 2, 3, 4, 4, 4]);

    // Same size: no-op.
    let mut buf = vec![1u8, 2, 3, 4];
    let orig = buf.clone();
    palette::extend_palette_color_map(&mut buf, 2, 2, 2, 2);
    assert_eq!(buf, orig);
}

#[test]
fn delta_encode_bits_hand_vectors() {
    // num=0: 0 bits.
    assert_eq!(palette::delta_encode_bits(&[], 8, 1), 0);
    // num=1: just the literal.
    assert_eq!(palette::delta_encode_bits(&[42], 10, 1), 10);

    // colors=[5,8,20], bit_depth=8, min_val=1.
    // literal=5(8b); deltas=[3,12], max_delta=12, min_bits=5,
    // bits_per_delta=max(ceil_log2(12),5)=max(4,5)=5, indicator=0(2b);
    // range=256-5-1=250 -> step(delta=3,val=2,bits=5) -> range=247,
    // bits_per_delta=min(5,ceil_log2(247)=8)=5;
    // step(delta=12,val=11,bits=5).
    let steps = palette::delta_encode_steps(&[5, 8, 20], 8, 1);
    assert_eq!(
        steps,
        vec![
            palette::DeltaEncodeStep { value: 5, bits: 8 },
            palette::DeltaEncodeStep { value: 0, bits: 2 },
            palette::DeltaEncodeStep { value: 2, bits: 5 },
            palette::DeltaEncodeStep { value: 11, bits: 5 },
        ]
    );
    assert_eq!(palette::delta_encode_bits(&[5, 8, 20], 8, 1), 8 + 2 + 5 + 5);

    // colors=[0,200,201,202], bit_depth=8, min_val=1: exercises a mid-
    // sequence SHRINK of bits_per_delta (8 -> 6) after the first big delta.
    // literal=0(8b); deltas=[200,1,1], max_delta=200,
    // bits_per_delta=max(ceil_log2(200)=8,5)=8, indicator=3(2b);
    // range=256-0-1=255 -> step(200,val=199,bits=8) -> range=55,
    // bits_per_delta=min(8,ceil_log2(55)=6)=6;
    // step(1,val=0,bits=6) -> range=54, bits_per_delta=min(6,ceil_log2(54)=6)=6;
    // step(1,val=0,bits=6).
    let steps = palette::delta_encode_steps(&[0, 200, 201, 202], 8, 1);
    assert_eq!(
        steps,
        vec![
            palette::DeltaEncodeStep { value: 0, bits: 8 },
            palette::DeltaEncodeStep { value: 3, bits: 2 },
            palette::DeltaEncodeStep { value: 199, bits: 8 },
            palette::DeltaEncodeStep { value: 0, bits: 6 },
            palette::DeltaEncodeStep { value: 0, bits: 6 },
        ]
    );
    assert_eq!(palette::delta_encode_bits(&[0, 200, 201, 202], 8, 1), 8 + 2 + 8 + 6 + 6);
}

// =============================================================================
// Chunk 2: palette_color_index_context + color_map_wavefront.
// Hand-derived vectors (both static C functions), documented per-scenario.
// =============================================================================

/// Exhaustive self-consistency: every valid entry of the C lookup table
/// (svt_aom_palette_color_index_context_lookup, palette.c:608) against the
/// TWO distinct C assertions that reference it -- these are NOT the same
/// formula, and conflating them is a bug (caught by this test's first
/// draft, which wrongly claimed a blanket `entry == 9 - hash`):
/// - the edge path (`_on_edge`, palette.c:638-641) hardcodes `hash = 2` and
///   asserts `LOOKUP[2] == 0` directly -- NOT via `9 - hash` (9-2=7 != 0).
/// - the interior path (palette.c:738-739) asserts `9 - hash ==
///   LOOKUP[hash]` for whatever hash it computed. Reachable interior
///   hashes are exactly {5, 6, 7, 8} (derived by hand in the tests below,
///   one per merge/all-equal/all-distinct branch), and only THOSE entries
///   satisfy the `9 - hash` identity.
#[test]
fn palette_color_index_context_lookup_matches_c_asserts() {
    const LOOKUP: [i32; 9] = [-1, -1, 0, -1, -1, 4, 3, 2, 1];
    // Edge case: hash is always 2, and C asserts the looked-up ctx is 0.
    assert_eq!(LOOKUP[2], 0);
    // Interior case: every reachable hash satisfies ctx == 9 - hash.
    for hash in [5usize, 6, 7, 8] {
        assert_eq!(LOOKUP[hash], 9 - hash as i32, "hash={hash}");
    }
    // The remaining table slots (0, 1, 3, 4) are unreachable by either path
    // and are documented as invalid (-1) in the C source.
    for hash in [0usize, 1, 3, 4] {
        assert_eq!(LOOKUP[hash], -1, "hash={hash} should be the documented invalid marker");
    }
}

/// Edge case (exactly one neighbor: top row OR left column), all three
/// sub-branches (increment / unchanged / match), both orientations.
/// Hand-traced from `av1_fast_palette_color_index_context_on_edge`
/// (palette.c:612-643): ctx is always 0 (color_score=2, hash=2, lookup[2]=0).
#[test]
fn palette_color_index_context_edge_hand_vectors() {
    // Top row (has_left only), stride=4, 1x4 map: [10, 3, 10, 10].
    let map = [10u8, 3, 10, 10];
    // (0,1): neighbor=map[0]=10 > current=3 -> idx=3+1=4.
    assert_eq!(palette::palette_color_index_context(&map, 4, 0, 1, 16), (0, 4));
    // (0,2): neighbor=map[1]=3, current=map[2]=10; 3<10, 3!=10 -> idx=10 unchanged.
    assert_eq!(palette::palette_color_index_context(&map, 4, 0, 2, 16), (0, 10));
    // (0,3): neighbor=map[2]=10 == current=map[3]=10 -> idx=0 (match).
    assert_eq!(palette::palette_color_index_context(&map, 4, 0, 3, 16), (0, 0));

    // Left column (has_above only), stride=1, 4x1 map: [7, 2, 7, 7].
    let map = [7u8, 2, 7, 7];
    // (1,0): neighbor=map[0]=7 > current=map[1]=2 -> idx=2+1=3.
    assert_eq!(palette::palette_color_index_context(&map, 1, 1, 0, 16), (0, 3));
    // (2,0): neighbor=map[1]=2, current=map[2]=7; 2<7,2!=7 -> idx=7 unchanged.
    assert_eq!(palette::palette_color_index_context(&map, 1, 2, 0, 16), (0, 7));
    // (3,0): neighbor=map[2]=7 == current=map[3]=7 -> idx=0 (match).
    assert_eq!(palette::palette_color_index_context(&map, 1, 3, 0, 16), (0, 0));
}

/// Interior, all three neighbors distinct (left=5, top=3, topleft=9 at the
/// tested pixel) -> num_valid_colors=3, ctx=1 always (hash=8 regardless of
/// tie-break outcome, traced from palette.c:694-713).  Sort: swap(0,1)
/// fires (tie on score=2, left(5) > top(3)) giving order [top=3, left=5,
/// topleft=9]; the num_valid>2 pass makes no further swap (scores stay
/// [2,2,1]). Covers cumulative multi-increment (no match) and the
/// match-at-idx0 case.
#[test]
fn palette_color_index_context_interior_all_distinct_hand_vectors() {
    // 2x2 map laid out so (1,1)'s neighbors are left=5 (1,0), top=3 (0,1),
    // topleft=9 (0,0); the value AT (1,1) itself is the `current` under test.
    let stride = 2usize;
    let mk = |current: u8| -> Vec<u8> { vec![9, 3, 5, current] };

    // current=3 matches sorted slot0 (top, post-swap) -> idx=0.
    let map = mk(3);
    assert_eq!(palette::palette_color_index_context(&map, stride, 1, 1, 16), (1, 0));
    // current=7: greater than all three (3,5,9 all < 7? NO: 9>7) ->
    // idx0(3>7?no) idx1(5>7?no) idx2(9>7?yes)->idx=7+1=8.
    let map = mk(7);
    assert_eq!(palette::palette_color_index_context(&map, stride, 1, 1, 16), (1, 8));
    // current=1: less than all three -> cumulative +1 three times -> idx=1+3=4.
    let map = mk(1);
    assert_eq!(palette::palette_color_index_context(&map, stride, 1, 1, 16), (1, 4));
}

/// Interior, left==top only (merge case a) -> num_valid_colors=2, ctx=3
/// (hash=6). left=top=4, topleft=9; no 0/1 swap needed (scores [4,1] not
/// tied). Covers match-at-slot0, match-at-slot1, and no-match-cumulative.
#[test]
fn palette_color_index_context_interior_left_eq_top_hand_vectors() {
    let stride = 2usize;
    // layout: topleft=9 (0,0), top=4 (0,1), left=4 (1,0), current at (1,1).
    let mk = |current: u8| -> Vec<u8> { vec![9, 4, 4, current] };

    let map = mk(4);
    assert_eq!(palette::palette_color_index_context(&map, stride, 1, 1, 16), (3, 0));
    let map = mk(9);
    assert_eq!(palette::palette_color_index_context(&map, stride, 1, 1, 16), (3, 1));
    let map = mk(2);
    assert_eq!(palette::palette_color_index_context(&map, stride, 1, 1, 16), (3, 4));
}

/// Interior, left==topleft only (merge case b) -> num_valid_colors=2, ctx=2
/// (hash=7). left=topleft=6, top=2; no 0/1 swap (scores [3,2] not tied).
/// The current=2 case exercises increment-THEN-overwrite-on-match: idx0
/// (6>2) bumps color_new_idx to 3, then idx1 (2==2) OVERWRITES it to 1 --
/// the accumulated increment must be discarded, not added to.
#[test]
fn palette_color_index_context_interior_left_eq_topleft_hand_vectors() {
    let stride = 2usize;
    // layout: topleft=6 (0,0), top=2 (0,1), left=6 (1,0), current at (1,1).
    let mk = |current: u8| -> Vec<u8> { vec![6, 2, 6, current] };

    let map = mk(6);
    assert_eq!(palette::palette_color_index_context(&map, stride, 1, 1, 16), (2, 0));
    let map = mk(2);
    assert_eq!(palette::palette_color_index_context(&map, stride, 1, 1, 16), (2, 1));
    let map = mk(9);
    assert_eq!(palette::palette_color_index_context(&map, stride, 1, 1, 16), (2, 9));
}

/// Interior, top==topleft only (merge case c) -> num_valid_colors=2, ctx=2
/// (hash=7, same ctx as case b via a different route -- cross-checks that
/// the hash only depends on the SORTED score profile). top=topleft=7,
/// left=1; the 0/1 swap DOES fire here (scores[0]=2 < scores[1]=3), unlike
/// case a/b, so this specifically exercises the swap-into-slot-0 path.
#[test]
fn palette_color_index_context_interior_top_eq_topleft_hand_vectors() {
    let stride = 2usize;
    // layout: topleft=7 (0,0), top=7 (0,1), left=1 (1,0), current at (1,1).
    let mk = |current: u8| -> Vec<u8> { vec![7, 7, 1, current] };

    let map = mk(7);
    assert_eq!(palette::palette_color_index_context(&map, stride, 1, 1, 16), (2, 0));
    let map = mk(1);
    assert_eq!(palette::palette_color_index_context(&map, stride, 1, 1, 16), (2, 1));
    let map = mk(0);
    assert_eq!(palette::palette_color_index_context(&map, stride, 1, 1, 16), (2, 2));
}

/// Interior, all three neighbors equal -> num_valid_colors=1, ctx=4
/// (hash=5, the ONLY way to reach num_valid_colors=1: the nested merge at
/// palette.c:676-679 fires). The `num_valid_colors>1` re-sort block is
/// skipped entirely (num_valid_colors fails the `>1` guard).
#[test]
fn palette_color_index_context_interior_all_equal_hand_vectors() {
    let stride = 2usize;
    let mk = |current: u8| -> Vec<u8> { vec![5, 5, 5, current] };

    let map = mk(5);
    assert_eq!(palette::palette_color_index_context(&map, stride, 1, 1, 16), (4, 0));
    let map = mk(2);
    assert_eq!(palette::palette_color_index_context(&map, stride, 1, 1, 16), (4, 3));
    let map = mk(9);
    assert_eq!(palette::palette_color_index_context(&map, stride, 1, 1, 16), (4, 9));
}

/// Anti-diagonal traversal order, hand-derived from the loop bounds
/// (palette.c:762-763: `k` in `1..=rows+cols-2`, `j` from `min(k,cols-1)`
/// DOWN TO `max(0,k-rows+1)`, `i=k-j`) for three shapes, cross-checked for
/// the AV1 dependency property (every visited pixel's left/top/topleft
/// neighbors are either `(0,0)` or already visited earlier in the sequence).
#[test]
fn color_map_wavefront_traversal_order_hand_vectors() {
    fn visited(rows: usize, cols: usize) -> Vec<(usize, usize)> {
        // Palette size 1 so any color_map content passes the `< palette_size`
        // assert trivially -- content is irrelevant, only the (i,j) order
        // and count are under test here.
        let map = vec![0u8; rows * cols];
        let mut out = Vec::new();
        palette::color_map_wavefront(&map, cols, rows, cols, 1, |i, j, _ctx, _idx| out.push((i, j)));
        out
    }

    assert_eq!(visited(1, 4), vec![(0, 1), (0, 2), (0, 3)]);
    assert_eq!(visited(4, 1), vec![(1, 0), (2, 0), (3, 0)]);
    assert_eq!(visited(2, 2), vec![(0, 1), (1, 0), (1, 1)]);
    assert_eq!(
        visited(3, 3),
        vec![(0, 1), (1, 0), (0, 2), (1, 1), (2, 0), (1, 2), (2, 1), (2, 2)]
    );
    // Degenerate 1x1: only (0,0) exists, which is always excluded -> empty.
    assert_eq!(visited(1, 1), vec![]);

    // Dependency property, general shapes: every (i,j) after (0,0) must have
    // its left/top/topleft already available (either (0,0) itself or an
    // earlier entry in the visitation order).
    for &(rows, cols) in &[(3usize, 3usize), (5, 2), (2, 5), (4, 4), (6, 3)] {
        let order = visited(rows, cols);
        let mut seen = Vec::from([(0usize, 0usize)]);
        for &(i, j) in &order {
            if i > 0 {
                assert!(seen.contains(&(i - 1, j)), "top not yet visited at {rows}x{cols} ({i},{j})");
            }
            if j > 0 {
                assert!(seen.contains(&(i, j - 1)), "left not yet visited at {rows}x{cols} ({i},{j})");
            }
            if i > 0 && j > 0 {
                assert!(seen.contains(&(i - 1, j - 1)), "topleft not yet visited at {rows}x{cols} ({i},{j})");
            }
            seen.push((i, j));
        }
        assert_eq!(order.len(), rows * cols - 1, "{rows}x{cols} visited count");
    }
}

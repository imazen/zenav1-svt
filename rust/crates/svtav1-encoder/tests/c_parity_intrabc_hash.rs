//! Differential parity: the IntraBC hash table (`svtav1-encoder/src/
//! intrabc_hash.rs`) vs the REAL exported C functions (IBC chunk 4,
//! docs/ibc-port-map.md §D).
//!
//! C oracles (all EXPORTED T-symbols, driven through thin shims):
//!   - `svt_av1_get_crc32c_value_c`               (hash.c:55)
//!   - `svt_av1_generate_block_2x2_hash_value`    (hash_motion.c:153)
//!   - `svt_av1_generate_block_hash_value`        (hash_motion.c:192)
//!   - `svt_aom_rtime_alloc_svt_av1_hash_table_create` +
//!     `..._add_to_hash_map_by_row_with_precal_data` (hash_motion.c:101/218)
//!     read back via `svt_av1_hash_table_count` +
//!     `svt_av1_hash_get_first_iterator` (:140/:148) — bucket CONTENTS
//!     **AND ORDER** (insertion order is the DV cost-tie tie-break)
//!   - `svt_av1_get_block_hash_value`             (hash_motion.c:309)

use svtav1_cref as cref;
use svtav1_encoder::intrabc_hash as hash;

/// Deterministic xorshift64* PRNG (house pattern — c_parity.rs).
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
}

/// Screen-content-like test frame: flat regions + REPEATED tiles (hash
/// collisions on equal content — the whole point of the table) + noise.
/// `w`/`h` are the hashed (aligned) dims; `stride >= w` exercises the
/// stride/width split.
fn screen_frame(rng: &mut Rng, w: usize, h: usize, stride: usize) -> Vec<u8> {
    let mut pic = vec![0u8; stride * h];
    // Base: flat 128 (many identical flat blocks -> dense buckets).
    for y in 0..h {
        for x in 0..w {
            pic[y * stride + x] = 128;
        }
    }
    // A repeated 8x8 "glyph" stamped at tile-aligned positions (exact
    // repeats -> hash_value2 matches at many positions).
    let mut glyph = [[0u8; 8]; 8];
    for row in glyph.iter_mut() {
        for v in row.iter_mut() {
            *v = (rng.below(200) + 20) as u8;
        }
    }
    let mut y = 0;
    while y + 8 <= h {
        let mut x = 0;
        while x + 8 <= w {
            if rng.below(3) == 0 {
                for (dy, row) in glyph.iter().enumerate() {
                    for (dx, &v) in row.iter().enumerate() {
                        pic[(y + dy) * stride + x + dx] = v;
                    }
                }
            }
            x += 8;
        }
        y += 16;
    }
    // A band of unique noise (unique hashes -> singleton buckets).
    for yy in (h / 2)..(h / 2 + 8).min(h) {
        for x in 0..w {
            pic[yy * stride + x] = (rng.below(256)) as u8;
        }
    }
    pic
}

// ---------------------------------------------------------------------------
// CRC-32C
// ---------------------------------------------------------------------------

#[test]
fn c_parity_crc32c() {
    let mut rng = Rng(0x1BC0_4A54_0001);
    // The exact 16-byte shape the pyramid uses, plus a spread of lengths
    // and (mis)alignments to cover C's alignment-prologue + quadword loop.
    let backing: Vec<u8> = (0..4096).map(|_| rng.below(256) as u8).collect();
    let mut checked = 0u64;
    for len in [0usize, 1, 2, 3, 4, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 100, 1000] {
        for off in 0..8usize {
            let buf = &backing[off..off + len];
            assert_eq!(
                hash::crc32c(buf),
                cref::crc32c(buf),
                "crc32c diverges at len={len} off={off}"
            );
            checked += 1;
        }
    }
    assert!(checked > 100);
    // Known-answer lock (also asserted in the unit tests): the C table
    // build itself is what we're comparing against, so pin one absolute.
    assert_eq!(cref::crc32c(b"123456789"), 0xE306_9283);
}

// ---------------------------------------------------------------------------
// Frame-level pyramid
// ---------------------------------------------------------------------------

/// Valid-region equality of the 2x2 base + every pyramid level. C leaves
/// positions outside `x < w-size+1, y < h-size+1` untouched (malloc
/// garbage in production; zero-seeded on both sides here), so only the
/// valid region is compared — plus an explicit check that BOTH sides kept
/// the seed value outside it (documents the C contract).
#[test]
fn c_parity_frame_hash_pyramid() {
    let mut rng = Rng(0x1BC0_4A54_0002);
    for (w, h, stride) in [(64usize, 48usize, 64usize), (80, 64, 96), (128, 96, 128)] {
        let pic = screen_frame(&mut rng, w, h, stride);

        // 2x2 base.
        let c_base = cref::generate_block_2x2_hash(&pic, stride, w, h);
        let mut rs_base = vec![0u32; w * h];
        hash::generate_block_2x2_hash_value(&pic, stride, w, h, &mut rs_base);
        for y in 0..h - 1 {
            for x in 0..w - 1 {
                assert_eq!(
                    rs_base[y * w + x],
                    c_base[y * w + x],
                    "2x2 hash diverges at ({x},{y}) [{w}x{h} stride {stride}]"
                );
            }
        }
        // Both sides leave the last column/row at the seed (0).
        for y in 0..h {
            assert_eq!(rs_base[y * w + w - 1], 0);
            assert_eq!(c_base[y * w + w - 1], 0);
        }

        // Pyramid levels 4..=64 (past what production uses at max_hash=64,
        // to lock the recursion depth generally).
        let mut c_src = c_base;
        let mut rs_src = rs_base;
        let mut size = 4usize;
        while size <= 64 && size <= w && size <= h {
            let c_dst = cref::generate_block_hash(w, h, size, &c_src);
            let mut rs_dst = vec![0u32; w * h];
            hash::generate_block_hash_value(w, h, size, &rs_src, &mut rs_dst);
            for y in 0..h - size + 1 {
                for x in 0..w - size + 1 {
                    assert_eq!(
                        rs_dst[y * w + x],
                        c_dst[y * w + x],
                        "size-{size} hash diverges at ({x},{y}) [{w}x{h}]"
                    );
                }
            }
            c_src = c_dst;
            rs_src = rs_dst;
            size <<= 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Bucket contents AND order
// ---------------------------------------------------------------------------

/// Build the full table on both sides (all sizes, the generate_ibc_data
/// recipe) and compare EVERY bucket: count, entries, and entry ORDER.
/// Order is load-bearing: `svt_av1_intrabc_hash_search` walks the bucket
/// with a strict `<` compare, so the first-inserted of equal-cost
/// candidates wins (map §A.3 fact 5).
#[test]
fn c_parity_hash_table_buckets_and_order() {
    let mut rng = Rng(0x1BC0_4A54_0003);
    // (w, h, stride, max_hash, max_cand, disallow_4x4)
    // max_cand 64 (level 4/5) exercises the truncation ordering; 256 is
    // levels 1-3; small 3 forces heavy truncation.
    for &(w, h, stride, max_hash, max_cand, disallow_4x4) in &[
        (96usize, 64usize, 96usize, 64u8, 256u16, false),
        (96, 64, 112, 64, 64, true),
        (64, 64, 64, 8, 32, false), // levels 6/7 shape: hash 8, cand 32
        (80, 48, 80, 16, 3, false), // tiny cap: truncation order stress
    ] {
        let pic = screen_frame(&mut rng, w, h, stride);

        // Port side: the whole generate_ibc_data recipe in one call.
        let rs_table = hash::generate_ibc_data(&pic, stride, w, h, max_hash, max_cand, disallow_4x4);

        // C side: same recipe driven through the exported build fns
        // (mirrors generate_ibc_data md_config_process.c:585-617).
        let mut c_table = cref::CHashTable::new();
        let mut c_vals = [cref::generate_block_2x2_hash(&pic, stride, w, h), vec![0u32; w * h]];
        let mut src_idx = 0usize;
        let mut size = 4usize;
        while size <= usize::from(max_hash) {
            let dst_idx = 1 - src_idx;
            c_vals[dst_idx] = cref::generate_block_hash(w, h, size, &c_vals[src_idx]);
            if size != 4 || !disallow_4x4 {
                c_table.add(&c_vals[dst_idx], w, h, size, max_cand);
            }
            src_idx = dst_idx;
            size <<= 1;
        }

        // Compare EVERY bucket (contents + order). Also count non-empty
        // buckets + multi-entry buckets for anti-vacuity.
        let mut nonempty = 0u64;
        let mut multi = 0u64;
        let mut total_entries = 0u64;
        for hv1 in 0..hash::HASH_TABLE_BUCKETS as u32 {
            let rs_bucket = rs_table.bucket(hv1);
            let c_count = c_table.count(hv1);
            assert_eq!(
                rs_bucket.len(),
                c_count,
                "bucket {hv1:#x} count diverges [{w}x{h} hash{max_hash} cand{max_cand}]"
            );
            if c_count == 0 {
                continue;
            }
            nonempty += 1;
            total_entries += c_count as u64;
            if c_count > 1 {
                multi += 1;
            }
            let c_bucket = c_table.bucket(hv1);
            for (i, (rs_e, c_e)) in rs_bucket.iter().zip(c_bucket.iter()).enumerate() {
                assert_eq!(
                    (rs_e.x, rs_e.y, rs_e.hash_value2),
                    *c_e,
                    "bucket {hv1:#x} entry {i} diverges (ORDER is load-bearing)"
                );
            }
        }
        // Anti-vacuity: the screen frame must produce a real table — many
        // buckets, some crowded (repeats), lots of entries.
        assert!(nonempty > 100, "vacuous table: {nonempty} non-empty buckets");
        assert!(multi > 10, "no crowded buckets: repeats missing from fixture");
        assert!(total_entries > 1000, "only {total_entries} entries");
        // With the tiny cap, at least one bucket must have been truncated
        // at exactly the cap (drop-later semantics exercised).
        if max_cand == 3 {
            let mut capped = 0u64;
            for hv1 in 0..hash::HASH_TABLE_BUCKETS as u32 {
                if rs_table.count(hv1) == 3 {
                    capped += 1;
                }
            }
            assert!(capped > 0, "cap=3 never hit: truncation untested");
        }
    }
}

// ---------------------------------------------------------------------------
// Per-block query
// ---------------------------------------------------------------------------

/// `svt_av1_get_block_hash_value` vs C at random positions/sizes, plus the
/// internal-consistency check that the query hash equals the frame-array
/// hash at the same position (which the search relies on for hits).
#[test]
fn c_parity_block_hash_query() {
    let mut rng = Rng(0x1BC0_4A54_0004);
    let (w, h, stride) = (96usize, 64usize, 104usize);
    let pic = screen_frame(&mut rng, w, h, stride);

    // Frame arrays (port side, already proven == C above).
    let mut base = vec![0u32; w * h];
    hash::generate_block_2x2_hash_value(&pic, stride, w, h, &mut base);
    let mut cur = base;
    let mut other = vec![0u32; w * h];

    let mut bufs = hash::BlockHashBuffers::new();
    let mut checked = 0u64;
    let mut size = 4usize;
    while size <= 64 {
        hash::generate_block_hash_value(w, h, size, &cur, &mut other);
        core::mem::swap(&mut cur, &mut other);
        for _ in 0..40 {
            let x = rng.below((w - size + 1) as u64) as usize;
            let y = rng.below((h - size + 1) as u64) as usize;
            let src = &pic[y * stride + x..];
            let (rs_hv1, rs_hv2) = hash::get_block_hash_value(src, stride, size, &mut bufs);
            let (c_hv1, c_hv2) = cref::get_block_hash_value(src, stride, size);
            assert_eq!(
                (rs_hv1, rs_hv2),
                (c_hv1, c_hv2),
                "query diverges at ({x},{y}) size {size}"
            );
            // Consistency with the frame pyramid (the hit condition).
            assert_eq!(rs_hv2, cur[y * w + x], "query != frame array at ({x},{y}) size {size}");
            checked += 1;
        }
        size <<= 1;
    }
    assert_eq!(checked, 5 * 40);
}

//! IntraBC frame hash table — CRC-32C block hashing, the frame-level hash
//! pyramid, hierarchical bucket insertion, and the per-block hash query
//! (IBC chunk 4, `docs/ibc-port-map.md` §D).
//!
//! C sources (SVT-AV1 v4.2.0, verified line-for-line):
//! - `hash.c:15-76` — CRC-32C (Castagnoli, poly `0x82f63b78` reversed),
//!   table-driven software fallback `svt_av1_get_crc32c_value_c`. The RTCD
//!   SSE4.2 / ARM-CRC32 variants are bit-identical by construction (same
//!   polynomial); this port implements the table version byte-at-a-time,
//!   which produces the same CRC as C's quadword-at-a-time loop (CRC is
//!   chunking-invariant over the same byte sequence).
//! - `hash_motion.c:34-93` — 2×2 pixel gather + the identity (bd8) pack.
//!   The HBD xor pack (`get_xor_hash_value_hbd`) is intentionally NOT
//!   ported: the entire IBC hash path is 8-bit-forced (map §A.3 fact 1 —
//!   the frame build hashes the 8-bit `enhanced_pic` link and the query
//!   passes `use_highbitdepth = 0` hardcoded, av1me.c:1071).
//! - `hash_motion.c:153-216` — frame-level 2×2 base + N×N pyramid
//!   (`svt_av1_generate_block_2x2_hash_value` /
//!   `svt_av1_generate_block_hash_value`).
//! - `hash_motion.c:101-151, 218-307` — the bucket table
//!   (`1 << (16+3)` buckets), the capped append
//!   (`hash_table_add_to_table`, drop-later-never-replace), and the
//!   hierarchical coarse-to-fine insertion order
//!   (`svt_aom_rtime_alloc_svt_av1_add_to_hash_map_by_row_with_precal_
//!   data`) — the insertion order IS the DV cost-tie tie-break (first
//!   inserted wins strict-`<` compares in `svt_av1_intrabc_hash_search`),
//!   so it is byte-exactness-critical, not a perf detail.
//! - `hash_motion.c:309-385` — the per-block query
//!   (`svt_av1_get_block_hash_value`) with its ping-pong scratch buffers.
//! - `md_config_process.c:585-617` — the frame-level build driver
//!   (`generate_ibc_data`): 2×2 base, then per-size pyramid 4..=
//!   `max_block_size_hash`, adding each size to the table except 4×4 when
//!   `pic_disallow_4x4` (the size-4 hash array is STILL generated — it is
//!   the pyramid source for size 8).
//!
//! Every function here is differentially locked against the exported C
//! symbols in `tests/c_parity_intrabc_hash.rs` (values, bucket contents
//! AND order, query results).

use alloc::vec;
use alloc::vec::Vec;

// =============================================================================
// CRC-32C (hash.c:15-76)
// =============================================================================

/// C `POLY` (hash.c:15): CRC-32C (iSCSI/Castagnoli) polynomial, reversed.
const POLY: u32 = 0x82f6_3b78;

/// The byte-at-a-time table (`crc32c_table[0]` of hash.c:25-42). C builds
/// 8 tables for its quadword loop; only table 0 affects the mathematical
/// result (tables 1-7 are a speed transform), so this port builds table 0
/// alone, at compile time.
const CRC32C_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut n = 0usize;
    while n < 256 {
        let mut crc = n as u32;
        let mut k = 0;
        while k < 8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ POLY } else { crc >> 1 };
            k += 1;
        }
        table[n] = crc;
        n += 1;
    }
    table
};

/// C `svt_av1_get_crc32c_value_c` (hash.c:55-76). Byte-at-a-time over the
/// same table — identical output to C's alignment-prologue + quadword loop
/// (CRC over the same bytes is independent of processing chunk size).
pub fn crc32c(buf: &[u8]) -> u32 {
    let mut crc: u32 = 0xffff_ffff;
    for &b in buf {
        crc = CRC32C_TABLE[((crc ^ u32::from(b)) & 0xff) as usize] ^ (crc >> 8);
    }
    crc ^ 0xffff_ffff
}

// =============================================================================
// Hash-key composition (hash_motion.c:17-18, 59-76)
// =============================================================================

/// C `crc_bits` (hash_motion.c:17).
pub const CRC_BITS: u32 = 16;
/// C `block_size_bits` (hash_motion.c:18).
pub const BLOCK_SIZE_BITS: u32 = 3;
/// Bucket count: C `max_addr = 1 << (crc_bits + block_size_bits)`
/// (hash_motion.c:24/109).
pub const HASH_TABLE_BUCKETS: usize = 1 << (CRC_BITS + BLOCK_SIZE_BITS);

/// C `hash_block_size_to_index` (hash_motion.c:59-76). Returns `None` for
/// non-power-of-two / out-of-range sizes (C returns -1 and asserts).
#[inline]
pub fn hash_block_size_to_index(block_size: i32) -> Option<u32> {
    match block_size {
        4 => Some(0),
        8 => Some(1),
        16 => Some(2),
        32 => Some(3),
        64 => Some(4),
        128 => Some(5),
        _ => None,
    }
}

/// C `get_identity_hash_value` (hash_motion.c:78-82): the 2×2 bd8 base
/// "hash" is just the four pixels packed big-endian-wise into one u32.
#[inline]
fn identity_hash_value(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (u32::from(a) << 24) + (u32::from(b) << 16) + (u32::from(c) << 8) + u32::from(d)
}

// =============================================================================
// Frame-level hash pyramid (hash_motion.c:153-216)
// =============================================================================

/// C `svt_av1_generate_block_2x2_hash_value` (hash_motion.c:153-190), bd8
/// arm only (see module doc). Writes `dst[y * w + x]` for every 2×2
/// top-left `(x, y)` with `x < w-1`, `y < h-1`; positions in the last
/// column/row of the `w*h` array are left untouched (C leaves them as
/// malloc garbage — callers never read them).
///
/// `w`/`h` are C's `y_crop_width`/`y_crop_height` — at the
/// `generate_ibc_data` call site these are the ALIGNED picture dims
/// (`pcs->ppcs->aligned_width/height`; the `enhanced_pic` 8-bit link sets
/// `y_crop_* = width/height` of the padded source, pic_buffer_desc.c:510+).
pub fn generate_block_2x2_hash_value(pic: &[u8], stride: usize, w: usize, h: usize, dst: &mut [u32]) {
    debug_assert!(dst.len() >= w * h);
    let x_end = w - 2 + 1;
    let y_end = h - 2 + 1;
    for y in 0..y_end {
        for x in 0..x_end {
            let p = &pic[y * stride + x..];
            let p0 = p[0];
            let p1 = p[1];
            let p2 = p[stride];
            let p3 = p[stride + 1];
            dst[y * w + x] = identity_hash_value(p0, p1, p2, p3);
        }
    }
}

/// C `svt_av1_generate_block_hash_value` (hash_motion.c:192-216): the
/// N×N hash at `(x, y)` is CRC-32C over the four (N/2)-hashes at
/// `(x, y)`, `(x+N/2, y)`, `(x, y+N/2)`, `(x+N/2, y+N/2)` serialized as
/// 16 bytes — C casts the `uint32_t p[4]` array to bytes, i.e.
/// LITTLE-ENDIAN per-word byte order on every supported target; this port
/// uses `to_le_bytes` explicitly.
pub fn generate_block_hash_value(w: usize, h: usize, block_size: usize, src: &[u32], dst: &mut [u32]) {
    debug_assert!(src.len() >= w * h && dst.len() >= w * h);
    let x_end = w - block_size + 1;
    let y_end = h - block_size + 1;
    let src_size = block_size >> 1;
    for y in 0..y_end {
        for x in 0..x_end {
            let pos = y * w + x;
            let p = [
                src[pos],
                src[pos + src_size],
                src[pos + src_size * w],
                src[pos + src_size * w + src_size],
            ];
            let mut bytes = [0u8; 16];
            for (i, v) in p.iter().enumerate() {
                bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            dst[pos] = crc32c(&bytes);
        }
    }
}

// =============================================================================
// The hash table (hash_motion.h:27-35, hash_motion.c:101-151)
// =============================================================================

/// C `BlockHash` (hash_motion.h:27-31): one bucket entry — the ABSOLUTE
/// picture-pixel origin of a hashed block plus its full 32-bit CRC
/// (`hash_value2`; the bucket key `hash_value1` is implicit in which
/// bucket the entry lives in).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockHash {
    pub x: i16,
    pub y: i16,
    pub hash_value2: u32,
}

/// C `HashTable` (hash_motion.h:33-35): `1 << 19` buckets, each an
/// append-only vector. Append order is load-bearing (first-inserted wins
/// DV cost ties — module doc).
pub struct HashTable {
    buckets: Vec<Vec<BlockHash>>,
}

impl HashTable {
    /// C `svt_aom_rtime_alloc_svt_av1_hash_table_create`
    /// (hash_motion.c:101-113) — fresh empty table. (C's idempotent
    /// "clear if exists" arm maps to just constructing a new value.)
    pub fn new() -> Self {
        Self { buckets: vec![Vec::new(); HASH_TABLE_BUCKETS] }
    }

    /// C `svt_av1_hash_table_count` (hash_motion.c:140-146).
    #[inline]
    pub fn count(&self, hash_value1: u32) -> usize {
        self.buckets[hash_value1 as usize].len()
    }

    /// The bucket for `hash_value1`, in insertion order — C's
    /// `svt_av1_hash_get_first_iterator` + `svt_aom_iterator_increment`
    /// walk (hash_motion.c:148-151, av1me.c:1080-1086).
    #[inline]
    pub fn bucket(&self, hash_value1: u32) -> &[BlockHash] {
        &self.buckets[hash_value1 as usize]
    }

    /// C `hash_table_add_to_table` (hash_motion.c:115-138): append the
    /// entry unless the bucket already holds `max_cand_per_bucket`
    /// entries — later entries are silently DROPPED, never replace
    /// earlier ones.
    #[inline]
    fn add(&mut self, hash_value1: u32, entry: BlockHash, max_cand_per_bucket: u16) {
        let bucket = &mut self.buckets[hash_value1 as usize];
        if bucket.len() < usize::from(max_cand_per_bucket) {
            bucket.push(entry);
        }
    }
}

impl Default for HashTable {
    fn default() -> Self {
        Self::new()
    }
}

/// C `svt_aom_rtime_alloc_svt_av1_add_to_hash_map_by_row_with_precal_data`
/// (hash_motion.c:218-307): add every `block_size`-sized block position of
/// the frame to the table, in the hierarchical coarse-to-fine order that
/// maximizes spatial dispersion of the first entries per bucket.
///
/// Order semantics (the load-bearing part):
/// - per pass, positions are walked COLUMN BY COLUMN (`x` outer, `y`
///   inner — hash_motion.c:262-263), stepping by `step`;
/// - passes run the offset state machine `(0,0) → (s/2,0) → (0,s/2) →
///   (s/2,s/2) → halve step, restart at (s/2,0)` — the `(0,0)` offset
///   combination is visited ONLY in the very first pass
///   (hash_motion.c:285-303);
/// - loop runs `while step > 1` (the finest stride used is 2).
pub fn add_to_hash_map_by_row_with_precal_data(
    table: &mut HashTable,
    pic_hash: &[u32],
    pic_width: usize,
    pic_height: usize,
    block_size: usize,
    max_cand_per_bucket: u16,
) {
    let x_end = pic_width - block_size + 1;
    let y_end = pic_height - block_size + 1;

    let add_value = hash_block_size_to_index(block_size as i32)
        .expect("hash block size must be one of 4/8/16/32/64/128")
        << CRC_BITS;
    let crc_mask: u32 = (1 << CRC_BITS) - 1;

    let mut step = block_size;
    let mut x_offset = 0usize;
    let mut y_offset = 0usize;

    while step > 1 {
        let mut x_pos = x_offset;
        while x_pos < x_end {
            let mut y_pos = y_offset;
            while y_pos < y_end {
                let pos = y_pos * pic_width + x_pos;
                let hash_value1 = (pic_hash[pos] & crc_mask) + add_value;
                table.add(
                    hash_value1,
                    BlockHash {
                        x: x_pos as i16,
                        y: y_pos as i16,
                        hash_value2: pic_hash[pos],
                    },
                    max_cand_per_bucket,
                );
                y_pos += step;
            }
            x_pos += step;
        }

        // The offset/step state machine (hash_motion.c:285-303).
        if x_offset == 0 && y_offset == 0 {
            // State 0 -> State 1 (only ever runs when step == block_size).
            x_offset = step / 2;
        } else if x_offset == step / 2 && y_offset == 0 {
            // State 1 -> State 2.
            x_offset = 0;
            y_offset = step / 2;
        } else if x_offset == 0 && y_offset == step / 2 {
            // State 2 -> State 3.
            x_offset = step / 2;
        } else {
            debug_assert!(x_offset == step / 2 && y_offset == step / 2);
            // State 3 -> State 1 at the next-finer step.
            step /= 2;
            x_offset = step / 2;
            y_offset = 0;
        }
    }
}

// =============================================================================
// Per-block hash query (hash.h:30, hash_motion.c:309-385)
// =============================================================================

/// C `AOM_BUFFER_SIZE_FOR_BLOCK_HASH` (hash.h:30).
pub const AOM_BUFFER_SIZE_FOR_BLOCK_HASH: usize = 4096;

/// The ping-pong scratch of C's `IntraBcContext::hash_value_buffer[2]`
/// (coding_unit.h:139-141, malloc'd per `intra_bc_search` invocation at
/// mode_decision.c:3014-3016). Reusable across queries.
pub struct BlockHashBuffers {
    bufs: [Vec<u32>; 2],
}

impl BlockHashBuffers {
    pub fn new() -> Self {
        Self {
            bufs: [
                vec![0u32; AOM_BUFFER_SIZE_FOR_BLOCK_HASH],
                vec![0u32; AOM_BUFFER_SIZE_FOR_BLOCK_HASH],
            ],
        }
    }
}

impl Default for BlockHashBuffers {
    fn default() -> Self {
        Self::new()
    }
}

/// C `svt_av1_get_block_hash_value` (hash_motion.c:309-385), bd8 arm (the
/// one call site passes `use_highbitdepth = 0` hardcoded, av1me.c:1071):
/// computes the probed block's own `(hash_value1, hash_value2)` by
/// building the in-block 2×2 base grid then reducing it through the
/// ping-pong pyramid. `src` is the block's top-left pixel (C's
/// `x->plane[0].src.buf`), `block_size` its (square) pixel dimension.
pub fn get_block_hash_value(
    src: &[u8],
    stride: usize,
    block_size: usize,
    bufs: &mut BlockHashBuffers,
) -> (u32, u32) {
    let add_value = hash_block_size_to_index(block_size as i32)
        .expect("hash block size must be one of 4/8/16/32/64/128")
        << CRC_BITS;
    let crc_mask: u32 = (1 << CRC_BITS) - 1;

    // 2x2 subblock hash values of the current block (hash_motion.c:315-347).
    let mut sub_block_in_width = block_size >> 1;
    let mut y_pos = 0usize;
    while y_pos < block_size {
        let mut x_pos = 0usize;
        while x_pos < block_size {
            let pos = (y_pos >> 1) * sub_block_in_width + (x_pos >> 1);
            let p = &src[y_pos * stride + x_pos..];
            debug_assert!(pos < AOM_BUFFER_SIZE_FOR_BLOCK_HASH);
            bufs.bufs[0][pos] = identity_hash_value(p[0], p[1], p[stride], p[stride + 1]);
            x_pos += 2;
        }
        y_pos += 2;
    }

    // Pyramid reduction with ping-pong buffers (hash_motion.c:349-381).
    let mut src_sub_block_in_width = sub_block_in_width;
    sub_block_in_width >>= 1;

    let mut src_idx = 0usize;
    let mut dst_idx = 1 - src_idx;

    let mut sub_width = 4usize;
    while sub_width <= block_size {
        dst_idx = 1 - src_idx;

        let mut dst_pos = 0usize;
        for y in 0..sub_block_in_width {
            for x in 0..sub_block_in_width {
                let src_pos = (y << 1) * src_sub_block_in_width + (x << 1);
                debug_assert!(src_pos + src_sub_block_in_width + 1 < AOM_BUFFER_SIZE_FOR_BLOCK_HASH);
                let to_hash = [
                    bufs.bufs[src_idx][src_pos],
                    bufs.bufs[src_idx][src_pos + 1],
                    bufs.bufs[src_idx][src_pos + src_sub_block_in_width],
                    bufs.bufs[src_idx][src_pos + src_sub_block_in_width + 1],
                ];
                let mut bytes = [0u8; 16];
                for (i, v) in to_hash.iter().enumerate() {
                    bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
                }
                bufs.bufs[dst_idx][dst_pos] = crc32c(&bytes);
                dst_pos += 1;
            }
        }

        src_sub_block_in_width = sub_block_in_width;
        sub_block_in_width >>= 1;

        sub_width *= 2;
        src_idx = 1 - src_idx;
    }

    let crc = bufs.bufs[dst_idx][0];
    ((crc & crc_mask) + add_value, crc)
}

// =============================================================================
// Frame-level build driver (md_config_process.c:585-617)
// =============================================================================

/// C `generate_ibc_data` (md_config_process.c:585-617, static; gated at
/// the :946-951 call site on `allow_intrabc && max_block_size_hash != 0`):
/// build the frame hash table over the ALIGNED picture dims. The size-4
/// hash array is always generated (pyramid source for size 8) but only
/// ADDED to the table when 4×4 blocks are allowed.
pub fn generate_ibc_data(
    pic: &[u8],
    stride: usize,
    aligned_width: usize,
    aligned_height: usize,
    max_block_size_hash: u8,
    max_cand_per_bucket: u16,
    pic_disallow_4x4: bool,
) -> HashTable {
    let mut table = HashTable::new();
    let mut block_hash_values = [
        vec![0u32; aligned_width * aligned_height],
        vec![0u32; aligned_width * aligned_height],
    ];

    generate_block_2x2_hash_value(pic, stride, aligned_width, aligned_height, &mut block_hash_values[0]);

    let mut src_idx = 0usize;
    let mut size = 4usize;
    while size <= usize::from(max_block_size_hash) {
        let dst_idx = 1 - src_idx;
        // Split-borrow the ping-pong pair.
        let (a, b) = block_hash_values.split_at_mut(1);
        let (src_buf, dst_buf) = if src_idx == 0 {
            (&a[0], &mut b[0])
        } else {
            (&b[0], &mut a[0])
        };
        generate_block_hash_value(aligned_width, aligned_height, size, src_buf, dst_buf);
        if size != 4 || !pic_disallow_4x4 {
            let dst_ref = if dst_idx == 0 { &block_hash_values[0] } else { &block_hash_values[1] };
            add_to_hash_map_by_row_with_precal_data(
                &mut table,
                dst_ref,
                aligned_width,
                aligned_height,
                size,
                max_cand_per_bucket,
            );
        }
        src_idx = dst_idx;
        size <<= 1;
    }

    table
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CRC-32C known-answer tests (RFC 3720 / iSCSI vectors).
    #[test]
    fn crc32c_known_vectors() {
        // Standard CRC-32C test vector: "123456789" -> 0xE3069283.
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        // 32 zero bytes -> 0x8A9136AA (iSCSI test pattern).
        assert_eq!(crc32c(&[0u8; 32]), 0x8A91_36AA);
        // 32 0xFF bytes -> 0x62A8AB43.
        assert_eq!(crc32c(&[0xFFu8; 32]), 0x62A8_AB43);
        assert_eq!(crc32c(b""), 0);
    }

    /// The hierarchical insertion order for an 8×8 region with
    /// block_size 4 must match the worked example in hash_motion.c's
    /// comment (:246-256): 25 candidate positions, visit order
    /// 1..=25 laid out per the comment grid.
    #[test]
    fn insertion_order_matches_c_comment_example() {
        // 8x8 picture, block_size 4 -> x_end = y_end = 5.
        // Give every position a unique CRC so each lands in a distinct
        // bucket; then reconstruct the visit order from bucket contents.
        let w = 8usize;
        let h = 8usize;
        let mut pic_hash = vec![0u32; w * h];
        for y in 0..5 {
            for x in 0..5 {
                pic_hash[y * w + x] = (y * w + x) as u32; // unique low bits
            }
        }
        let mut table = HashTable::new();
        add_to_hash_map_by_row_with_precal_data(&mut table, &pic_hash, w, h, 4, 256);

        // Expected visit order from the C comment (x, y) grid:
        //    x  0  1  2  3  4
        //  y +---------------
        //  0 |  1 10  5 13  3
        //  1 | 16 22 18 24 20
        //  2 |  7 11  9 14  8
        //  3 | 17 23 19 25 21
        //  4 |  2 12  6 15  4
        let expect_order_grid: [[u32; 5]; 5] = [
            [1, 10, 5, 13, 3],
            [16, 22, 18, 24, 20],
            [7, 11, 9, 14, 8],
            [17, 23, 19, 25, 21],
            [2, 12, 6, 15, 4],
        ];

        // Reconstruct: order[k] = (x, y) visited at step k+1. Every bucket
        // has exactly one entry; recover the global order by walking the
        // state machine ourselves is circular, so instead insert into ONE
        // bucket: rebuild with all-equal hashes and cap large enough.
        let mut one_bucket_hash = vec![0u32; w * h];
        for v in one_bucket_hash.iter_mut() {
            *v = 0x1234_5678;
        }
        let mut t2 = HashTable::new();
        add_to_hash_map_by_row_with_precal_data(&mut t2, &one_bucket_hash, w, h, 4, 256);
        let hv1 = (0x1234_5678u32 & 0xffff) + (0 << 16);
        let bucket = t2.bucket(hv1);
        assert_eq!(bucket.len(), 25);
        for (k, e) in bucket.iter().enumerate() {
            let step = (k + 1) as u32;
            assert_eq!(
                expect_order_grid[e.y as usize][e.x as usize], step,
                "position ({}, {}) inserted at step {} but C comment says {}",
                e.x, e.y, step, expect_order_grid[e.y as usize][e.x as usize]
            );
        }
        let _ = table;
    }

    /// Bucket cap: later entries dropped, never replacing earlier ones.
    #[test]
    fn bucket_cap_drops_later() {
        let w = 8usize;
        let h = 8usize;
        let pic_hash = vec![0xABCDu32; w * h];
        let mut table = HashTable::new();
        add_to_hash_map_by_row_with_precal_data(&mut table, &pic_hash, w, h, 4, 3);
        let hv1 = (0xABCDu32 & 0xffff) + 0;
        let bucket = table.bucket(hv1);
        assert_eq!(bucket.len(), 3);
        // First three of the hierarchical order for the 5x5 grid:
        // step 1 = (0,0), step 2 = (0,4), step 3 = (4,0).
        assert_eq!((bucket[0].x, bucket[0].y), (0, 0));
        assert_eq!((bucket[1].x, bucket[1].y), (0, 4));
        assert_eq!((bucket[2].x, bucket[2].y), (4, 0));
    }

    /// The query's own pyramid must agree with the frame-level pyramid at
    /// every position (C guarantees this by construction — same math).
    #[test]
    fn query_matches_frame_pyramid() {
        let w = 32usize;
        let h = 24usize;
        let stride = 40usize;
        let mut pic = vec![0u8; stride * h];
        let mut state = 0x1234_5678_9abc_def0u64;
        for y in 0..h {
            for x in 0..w {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                pic[y * stride + x] = (state >> 33) as u8;
            }
        }
        let mut base = vec![0u32; w * h];
        generate_block_2x2_hash_value(&pic, stride, w, h, &mut base);
        let mut cur = base;
        let mut other = vec![0u32; w * h];
        let mut bufs = BlockHashBuffers::new();
        for size in [4usize, 8, 16] {
            generate_block_hash_value(w, h, size, &cur, &mut other);
            core::mem::swap(&mut cur, &mut other);
            let add_value = hash_block_size_to_index(size as i32).unwrap() << CRC_BITS;
            for y in (0..h - size + 1).step_by(5) {
                for x in (0..w - size + 1).step_by(3) {
                    let (hv1, hv2) = get_block_hash_value(&pic[y * stride + x..], stride, size, &mut bufs);
                    assert_eq!(hv2, cur[y * w + x], "hv2 mismatch at ({x},{y}) size {size}");
                    assert_eq!(hv1, (cur[y * w + x] & 0xffff) + add_value);
                }
            }
        }
    }
}

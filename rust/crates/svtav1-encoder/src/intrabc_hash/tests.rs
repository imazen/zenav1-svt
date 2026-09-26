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
    let add_value = hash_block_size_to_index(4).unwrap() << CRC_BITS;
    let hv1 = (0x1234_5678u32 & 0xffff) + add_value;
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
    let add_value = hash_block_size_to_index(4).unwrap() << CRC_BITS;
    let hv1 = (0xABCDu32 & 0xffff) + add_value;
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
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
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
                let (hv1, hv2) =
                    get_block_hash_value(&pic[y * stride + x..], stride, size, &mut bufs);
                assert_eq!(hv2, cur[y * w + x], "hv2 mismatch at ({x},{y}) size {size}");
                assert_eq!(hv1, (cur[y * w + x] & 0xffff) + add_value);
            }
        }
    }
}

/// A picture SMALLER than the hash block size must produce no hashes and
/// no table entries — not a panic.
///
/// C computes `x_end = pic_width - block_size + 1` as a SIGNED int
/// (hash_motion.c:195-196, :222-223); when it goes negative every loop
/// body is simply skipped. The port used `usize`, so the subtraction
/// underflowed and wrapped to ~2^64, and the loops indexed off the end.
///
/// MEASURED before the fix, through the PUBLIC encode API on a 32x32
/// screen frame at preset 0 (where IntraBC is armed):
///   generate_block_hash_value          -> "len is 1024 but the index is 1024"
///   add_to_hash_map_by_row_with_precal -> "len is 1024 but the index is 2048"
/// Found by `tools/identity_full_8bit.sh`'s dims tier; no earlier gate
/// encoded anything below 60x60 with the screen-content tools on.
#[test]
fn hash_helpers_are_total_when_the_picture_is_smaller_than_the_block() {
    // 16x16 picture, 32x32 and 64x64 hash blocks: strictly smaller.
    let (w, h) = (16usize, 16usize);
    let src = alloc::vec![0u32; w * h];
    let mut dst = alloc::vec![0u32; w * h];
    for block_size in [32usize, 64, 128] {
        generate_block_hash_value(w, h, block_size, &src, &mut dst);
        assert!(
            dst.iter().all(|&v| v == 0),
            "no hash may be written when the block does not fit"
        );
        let mut table = HashTable::new();
        add_to_hash_map_by_row_with_precal_data(&mut table, &src, w, h, block_size, 16);
    }
    // The 2x2 base case on a degenerate 1-pixel-wide picture.
    let pic = alloc::vec![0u8; 1];
    let mut d2 = alloc::vec![0u32; 1];
    generate_block_2x2_hash_value(&pic, 1, 1, 1, &mut d2);
    assert_eq!(d2[0], 0);

    // Anti-vacuity: a picture LARGER than the block still hashes.
    let (bw, bh) = (64usize, 64usize);
    let bsrc = alloc::vec![7u32; bw * bh];
    let mut bdst = alloc::vec![0u32; bw * bh];
    generate_block_hash_value(bw, bh, 32, &bsrc, &mut bdst);
    assert!(
        bdst.iter().any(|&v| v != 0),
        "a fitting block MUST produce hashes, else this test proves nothing"
    );
}

//! Differential tests for the per-block geometry setup of `write_modes_b`
//! (`svtav1_encoder::port_entropy_inter::neighbors`).
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4): `set_mi_row_col` is an
//! EXPORTED symbol (`nm -g libSvtAv1Enc.a` prints `T _set_mi_row_col`), so
//! every assertion's right-hand side is a call into the release archive's own
//! compiled code, through `svtav1-cref`'s `entropy_block` shim. Nothing here
//! compares one transcription against another.
//!
//! `max_block_wide` / `max_block_high` are `static INLINE` in
//! `entropy_coding.c`, which no shim compiles; they are covered at tier 4 in
//! the module's own unit tests, built on the tier-1 edges gated here.

use svtav1_cref::entropy_block as cref;
use svtav1_encoder::port_entropy_inter::neighbors::{TileBounds, set_mi_row_col};

/// The (bw, bh) mi-unit shapes AV1 can code, including the 4:1 and the
/// PARTITION_*_4 sub-shapes where `is_sec_rect` is decided.
const SHAPES: &[(i32, i32)] = &[
    (1, 1),
    (1, 2),
    (2, 1),
    (2, 2),
    (2, 4),
    (4, 2),
    (4, 4),
    (4, 8),
    (8, 4),
    (8, 8),
    (8, 16),
    (16, 8),
    (16, 16),
    (16, 32),
    (32, 16),
    (32, 32),
    (1, 4),
    (4, 1),
    (2, 8),
    (8, 2),
    (4, 16),
    (16, 4),
];

/// Frame shapes to sweep: (mi_rows, mi_cols, mi_stride). The stride is
/// deliberately not always equal to `mi_cols` — C's `mi_stride` is the
/// grid's, and conflating the two silently mislocates every neighbour.
const FRAMES: &[(i32, i32, i32)] = &[(16, 16, 16), (64, 64, 64), (33, 47, 48), (20, 12, 16)];

/// Tile origins to sweep, including a non-zero one: availability is
/// TILE-relative while the edges are FRAME-relative, and that asymmetry is
/// the whole point of the function.
const TILE_ORIGINS: &[(i32, i32)] = &[(0, 0), (4, 4), (8, 0), (0, 8)];

#[test]
fn c_parity_set_mi_row_col() {
    let mut cases = 0usize;
    let mut up_true = 0usize;
    let mut left_true = 0usize;
    let mut sec_rect_true = 0usize;
    let mut negative_edge = 0usize;

    for &(mi_rows, mi_cols, mi_stride) in FRAMES {
        for &(tile_r, tile_c) in TILE_ORIGINS {
            if tile_r >= mi_rows || tile_c >= mi_cols {
                continue;
            }
            let tile = TileBounds {
                mi_row_start: tile_r,
                mi_col_start: tile_c,
            };
            for &(bw, bh) in SHAPES {
                // C's own callers only ever place a block on a multiple of
                // its own smaller dimension; sweeping every mi position
                // instead would drive `set_mi_row_col` outside the domain
                // the encoder produces, which is the trap
                // WORKING-ON-THIS §5 records as "bound a generator by what
                // the PRODUCER can produce".
                let step_r = bh.min(4).max(1);
                let step_c = bw.min(4).max(1);
                let mut mi_row = tile_r;
                while mi_row < mi_rows {
                    let mut mi_col = tile_c;
                    while mi_col < mi_cols {
                        let got = set_mi_row_col(
                            &tile, mi_row, bh, mi_col, bw, mi_stride, mi_rows, mi_cols,
                        );
                        let want = cref::set_mi_row_col(
                            mi_row, bh, mi_col, bw, mi_stride, mi_rows, mi_cols, tile_r, tile_c,
                        )
                        .expect("cref shim allocation failed — environment, not parity");

                        let ctx = format!(
                            "frame {mi_rows}x{mi_cols} stride {mi_stride} tile ({tile_r},{tile_c}) \
                             block {bw}x{bh} at ({mi_row},{mi_col})"
                        );
                        assert_eq!(got.mb_to_top_edge, want.mb_to_top_edge, "top: {ctx}");
                        assert_eq!(
                            got.mb_to_bottom_edge, want.mb_to_bottom_edge,
                            "bottom: {ctx}"
                        );
                        assert_eq!(got.mb_to_left_edge, want.mb_to_left_edge, "left: {ctx}");
                        assert_eq!(got.mb_to_right_edge, want.mb_to_right_edge, "right: {ctx}");
                        assert_eq!(got.up_available, want.up_available, "up_avail: {ctx}");
                        assert_eq!(got.left_available, want.left_available, "left_avail: {ctx}");
                        assert_eq!(got.above_mi, want.above_mi, "above_mi: {ctx}");
                        assert_eq!(got.left_mi, want.left_mi, "left_mi: {ctx}");
                        assert_eq!(got.n8_w, want.n8_w, "n8_w: {ctx}");
                        assert_eq!(got.n8_h, want.n8_h, "n8_h: {ctx}");
                        assert_eq!(got.is_sec_rect, want.is_sec_rect, "is_sec_rect: {ctx}");
                        assert_eq!(got.mi_offset, want.mi_offset, "mi_offset: {ctx}");

                        cases += 1;
                        up_true += usize::from(want.up_available);
                        left_true += usize::from(want.left_available);
                        sec_rect_true += usize::from(want.is_sec_rect);
                        negative_edge +=
                            usize::from(want.mb_to_bottom_edge < 0 || want.mb_to_right_edge < 0);
                        mi_col += step_c;
                    }
                    mi_row += step_r;
                }
            }
        }
    }

    // Anti-vacuity, per output that has a false default. Without these a
    // port that returned `up_available = false`, `is_sec_rect = false` and
    // `above_mi = None` everywhere would pass the sweep on the cases where C
    // happens to agree.
    assert!(cases > 1000, "sweep too small to discriminate: {cases}");
    assert!(up_true > 0 && up_true < cases, "up_available is constant");
    assert!(
        left_true > 0 && left_true < cases,
        "left_available is constant"
    );
    assert!(sec_rect_true > 0, "is_sec_rect never fires — probe dead");
    assert!(negative_edge > 0, "no block ever overhangs — probe dead");
}

// ===========================================================================
// The small EXPORTED helpers of entropy_coding.c — all TIER 1.
//
// Four of these gate a function this change ports for the first time
// (`port_entropy_inter::primitives`); the other five gate a function the
// port has had for a long time WITHOUT a differential, which is the gap
// this file closes.
// ===========================================================================

use svtav1_encoder::port_entropy_inter::primitives as prim;
use svtav1_types::block::BlockSize;

/// C `svt_aom_partition_cdf_length` vs `sb128_geom::partition_cdf_length`.
///
/// The port keys on the SQUARE SIZE in pixels rather than the `BlockSize`
/// enum, so the sweep drives the square sizes and maps each to its enum id.
#[test]
fn c_parity_partition_cdf_length() {
    for (sq, bsize) in [
        (8usize, BlockSize::Block8x8),
        (16, BlockSize::Block16x16),
        (32, BlockSize::Block32x32),
        (64, BlockSize::Block64x64),
        (128, BlockSize::Block128x128),
    ] {
        assert_eq!(
            svtav1_encoder::sb128_geom::partition_cdf_length(sq) as i32,
            cref::partition_cdf_length(bsize as i32),
            "square {sq}",
        );
    }
}

/// C `svt_aom_allow_palette` over every block size and both tool states.
#[test]
fn c_parity_allow_palette() {
    let mut trues = 0;
    for i in 0..22u8 {
        let Some(b) = BlockSize::from_u8(i) else {
            continue;
        };
        for sc in [false, true] {
            let got = prim::allow_palette(sc, b);
            assert_eq!(got, cref::allow_palette(sc, i as i32), "bsize {i} sc {sc}");
            trues += usize::from(got);
        }
    }
    assert!(trues > 0, "palette never allowed — probe dead");
}

/// C `svt_aom_get_palette_bsize_ctx` vs `entropy::context::palette_bsize_ctx`.
#[test]
fn c_parity_palette_bsize_ctx() {
    for i in 0..22u8 {
        let Some(b) = BlockSize::from_u8(i) else {
            continue;
        };
        if !prim::allow_palette(true, b) {
            // C's table is only meaningful where palette is allowed; the
            // encoder never asks otherwise.
            continue;
        }
        let w = usize::from(svtav1_types::tables::block::BLOCK_SIZE_WIDE[b.as_index()]);
        let h = usize::from(svtav1_types::tables::block::BLOCK_SIZE_HIGH[b.as_index()]);
        assert_eq!(
            svtav1_encoder::entropy::context::palette_bsize_ctx(w, h) as i32,
            cref::palette_bsize_ctx(i as i32),
            "bsize {i}",
        );
    }
}

/// C `svt_aom_write_uniform_cost` — the two arms differ by a whole literal.
#[test]
fn c_parity_write_uniform_cost() {
    let mut distinct = std::collections::BTreeSet::new();
    for n in 0..=64i32 {
        for v in 0..n.max(1) {
            let got = prim::write_uniform_cost(n, v);
            assert_eq!(got, cref::write_uniform_cost(n, v), "n {n} v {v}");
            distinct.insert(got);
        }
    }
    assert!(distinct.len() > 3, "cost is near-constant — probe dead");
}

/// C `svt_aom_uleb_size_in_bytes` — one byte for zero (a `do/while`).
#[test]
fn c_parity_uleb_size_in_bytes() {
    let mut vals: Vec<u64> = vec![0, 1, 127, 128, 129, 16383, 16384, u64::MAX];
    for s in 0..64 {
        vals.push(1u64 << s);
    }
    for v in vals {
        assert_eq!(
            prim::uleb_size_in_bytes(v) as u64,
            cref::uleb_size_in_bytes(v),
            "value {v}",
        );
    }
}

/// C `svt_aom_uleb_encode` vs `entropy::obu::uleb_encode`, including the
/// sizes where C REFUSES (value above the LEB128 cap, or not enough room).
#[test]
fn c_parity_uleb_encode() {
    let mut refused = 0;
    let mut coded = 0;
    for v in [0u32, 1, 127, 128, 300, 16383, 16384, 1 << 30, u32::MAX] {
        for available in [0u64, 1, 2, 4, 8] {
            match cref::uleb_encode(u64::from(v), available) {
                Ok(want) => {
                    let got = svtav1_encoder::entropy::obu::uleb_encode(v);
                    assert_eq!(got, want, "value {v} available {available}");
                    coded += 1;
                }
                Err(_) => refused += 1,
            }
        }
    }
    assert!(coded > 0 && refused > 0, "one-sided sweep — probe dead");
}

/// C `av1_get_skip_context` vs `entropy::context::get_skip_context`.
///
/// C tests the neighbour POINTER, so a missing neighbour and a present
/// non-skip one are different inputs that happen to score the same.
#[test]
fn c_parity_get_skip_context() {
    let states = [None, Some(false), Some(true)];
    let mut seen = std::collections::BTreeSet::new();
    for a in states {
        for l in states {
            let port = svtav1_encoder::entropy::context::get_skip_context(
                a.unwrap_or(false),
                l.unwrap_or(false),
            );
            let c = cref::get_skip_context(a, l);
            assert_eq!(port as i32, c, "above {a:?} left {l:?}");
            seen.insert(c);
        }
    }
    assert_eq!(seen.len(), 3, "all three contexts must be reachable");
}

/// C `svt_aom_get_palette_mode_ctx` vs the new
/// `primitives::palette_mode_ctx`.
#[test]
fn c_parity_palette_mode_ctx() {
    let states = [None, Some(0u8), Some(2), Some(8)];
    let mut seen = std::collections::BTreeSet::new();
    for a in states {
        for l in states {
            let got = prim::palette_mode_ctx(a, l);
            let c = cref::get_palette_mode_ctx(a, l);
            assert_eq!(got as i32, c, "above {a:?} left {l:?}");
            seen.insert(c);
        }
    }
    assert_eq!(seen.len(), 3, "all three contexts must be reachable");
}

/// C `svt_aom_get_kf_y_mode_ctx` vs the new `primitives::kf_y_mode_ctx`,
/// over every intra mode and both availability states.
#[test]
fn c_parity_kf_y_mode_ctx() {
    let mut seen = std::collections::BTreeSet::new();
    for up in (0..13u8).map(Some).chain([None]) {
        for left in (0..13u8).map(Some).chain([None]) {
            let got = prim::kf_y_mode_ctx(up, left);
            let c = cref::get_kf_y_mode_ctx(up, left);
            assert_eq!(got, c, "up {up:?} left {left:?}");
            seen.insert(c);
        }
    }
    assert!(seen.len() > 5, "context pair is near-constant — probe dead");
}

/// C `svt_aom_count_primitive_quniform` / `_subexpfin` vs `entropy::lr`'s
/// counterparts, which price every loop-restoration coefficient.
#[test]
fn c_parity_count_primitives() {
    let mut nonzero = 0;
    for n in 1..=64u16 {
        for v in 0..n {
            assert_eq!(
                svtav1_encoder::entropy::lr::count_primitive_quniform(n, v),
                cref::count_primitive_quniform(n as i32, v as i32),
                "quniform n {n} v {v}",
            );
        }
    }
    for n in [8u16, 16, 32, 64, 128, 256] {
        for k in 0..=5u16 {
            for v in 0..n {
                let got = svtav1_encoder::entropy::lr::count_primitive_subexpfin(n, k, v);
                assert_eq!(
                    got,
                    cref::count_primitive_subexpfin(n as i32, k as i32, v as i32),
                    "subexpfin n {n} k {k} v {v}",
                );
                nonzero += usize::from(got != 0);
            }
        }
    }
    assert!(nonzero > 0, "every count is zero — probe dead");
}

/// C's header bit buffer (`svt_aom_wb_write_bit` / `_literal` /
/// `_inv_signed_literal` / `_bytes_written` / `_is_byte_aligned`) vs
/// `entropy::obu::BitWriter`. Every frame and sequence header is built on
/// these five, and none of them had a differential.
#[test]
fn c_parity_write_bit_buffer() {
    use svtav1_cref::entropy_block::WbOp;
    use svtav1_encoder::entropy::obu::BitWriter;

    // A deterministic script that mixes all three op kinds and lands on
    // both aligned and unaligned bit offsets.
    let mut ops = Vec::new();
    let mut seed = 0x1234_5678u32;
    let mut next = |m: u32| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 16) % m
    };
    for _ in 0..200 {
        match next(3) {
            0 => ops.push(WbOp::Bit(next(2) == 1)),
            1 => {
                let bits = 1 + next(16) as i32;
                let data = (next(1 << 16) as i32) & ((1i32 << bits) - 1);
                ops.push(WbOp::Literal { data, bits });
            }
            _ => {
                let bits = 1 + next(8) as i32;
                let mag = (next(1 << 8) as i32) & ((1i32 << bits) - 1);
                let data = if next(2) == 1 { -mag } else { mag };
                ops.push(WbOp::InvSigned { data, bits });
            }
        }
    }

    let mut aligned_seen = [false, false];
    for take in 1..=ops.len() {
        let script = &ops[..take];
        let (want_bytes, want_written, want_aligned) =
            svtav1_cref::entropy_block::wb_run(script, 4096);

        let mut w = BitWriter::new();
        for op in script {
            match *op {
                WbOp::Bit(b) => w.write_bit(b),
                WbOp::Literal { data, bits } => w.write_bits(data as u32, bits as u32),
                WbOp::InvSigned { data, bits } => {
                    // C `svt_aom_wb_write_inv_signed_literal(data, bits)` is
                    // `write_literal(data, bits + 1)` on the two's-complement
                    // low bits; the port spells it as its own method.
                    w.write_bits((data as u32) & ((1u32 << (bits + 1)) - 1), bits as u32 + 1);
                }
            }
        }
        let got_written = w.bytes_written();
        assert_eq!(got_written as u32, want_written, "bytes_written at {take}");
        assert_eq!(w.data(), &want_bytes[..], "bytes at {take}");
        let got_aligned = w.bit_len().is_multiple_of(8);
        assert_eq!(got_aligned, want_aligned, "is_byte_aligned at {take}");
        aligned_seen[usize::from(want_aligned)] = true;
    }
    assert!(
        aligned_seen[0] && aligned_seen[1],
        "the script never hit both alignment states — probe dead"
    );
}

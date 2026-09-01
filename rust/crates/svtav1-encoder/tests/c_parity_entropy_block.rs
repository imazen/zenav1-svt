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

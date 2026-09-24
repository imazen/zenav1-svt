//! Differential parity for `port_tpl`'s open-loop neighbour fill
//! (`update_neighbor_samples_open_loop`) vs C
//! `svt_aom_update_neighbor_samples_array_open_loop_mb_recon`
//! (enc_intra_prediction.c:818) — evidence tier 1.
//!
//! The non-`_recon` C entry is the same body modulo reading
//! `input_ptr->y_buffer`/`width`/`height`, so parity here covers both call
//! shapes the TPL dispenser makes (source path and TPL-recon path).
//!
//! The corner/edge arms are where this function earns its keep: the
//! `x==0`/`y==0` branches take pixel values from the *other* edge's array
//! through C's pointer arithmetic — the arm most likely to be mistranslated.

use svtav1_cref::pic_operators as cref_po;
use svtav1_encoder::port_tpl::{get_neighbor_samples_dc, update_neighbor_samples_open_loop};

const W: usize = 96;
const H: usize = 80;
const STRIDE: usize = 128;

fn frame() -> Vec<u8> {
    (0..STRIDE * H)
        .map(|i| ((i * 37 + (i / STRIDE) * 91 + 13) & 0xff) as u8)
        .collect()
}

/// above/left in the [0]=corner convention, length `1 + 2*bsize` — the max
/// `use_trbl` writes.
fn run_port(
    buf: &[u8],
    trbl: bool,
    top: bool,
    x: u32,
    y: u32,
    bw: u8,
    bh: u8,
) -> (Vec<u8>, Vec<u8>) {
    let mut above = vec![0xAAu8; 1 + 2 * bw as usize];
    let mut left = vec![0xAAu8; 1 + 2 * bh as usize];
    update_neighbor_samples_open_loop(
        trbl,
        top,
        buf,
        STRIDE,
        W as u32,
        H as u32,
        x,
        y,
        bw as usize,
        bh as usize,
        &mut above,
        &mut left,
    );
    (above, left)
}

fn run_c(buf: &[u8], trbl: bool, top: bool, x: u32, y: u32, bw: u8, bh: u8) -> (Vec<u8>, Vec<u8>) {
    let mut above = vec![0xAAu8; 1 + 2 * bw as usize];
    let mut left = vec![0xAAu8; 1 + 2 * bh as usize];
    cref_po::update_neighbor_samples_open_loop_recon(
        trbl, top, &mut above, &mut left, buf, STRIDE, x, y, bw, bh, W as u32, H as u32,
    );
    (above, left)
}

#[test]
fn neighbor_samples_open_loop_matches_c() {
    let buf = frame();
    // (x, y): interior, left edge, top edge, top-left corner, right-edge
    // (x + 2*bw > width), bottom-edge, and corner-adjacent positions.
    let positions: &[(u32, u32)] = &[
        (0, 0),
        (16, 0),
        (48, 0),
        (0, 16),
        (0, 48),
        (16, 16),
        (48, 32),
        (64, 16), // x + 2*32 > 96 for bsize 32 — right-edge clamp arm
        (16, 64), // y + 2*16 > 80 for bsize 16 — bottom-edge clamp arm
        (0, 64),
        (64, 64),
        (8, 8), // non-16-aligned origin
    ];
    for &(x, y) in positions {
        for &bs in &[8u8, 16, 32] {
            if x as usize + bs as usize > STRIDE || y as usize + bs as usize > H {
                continue;
            }
            for trbl in [true, false] {
                for top in [true, false] {
                    let (pa, pl) = run_port(&buf, trbl, top, x, y, bs, bs);
                    let (ca, cl) = run_c(&buf, trbl, top, x, y, bs, bs);
                    assert_eq!(pa, ca, "above: ({x},{y}) bs={bs} trbl={trbl} top={top}");
                    assert_eq!(pl, cl, "left:  ({x},{y}) bs={bs} trbl={trbl} top={top}");
                }
            }
        }
    }
}

/// `get_neighbor_samples_dc` (src_ops_process.c:359): direct pixel copy of
/// the block's real above/left neighbours, corner at index 0.
#[test]
fn neighbor_samples_dc_copies_real_edge() {
    let buf = frame();
    let (x, y, bs) = (32usize, 16usize, 16usize);
    let origin = y * STRIDE + x;
    let mut above = vec![0u8; 1 + bs];
    let mut left = vec![0u8; 1 + bs];
    get_neighbor_samples_dc(&buf, STRIDE, origin, &mut above, &mut left, bs);
    assert_eq!(above[0], buf[origin - STRIDE - 1]);
    assert_eq!(left[0], buf[origin - STRIDE - 1]);
    for i in 0..bs {
        assert_eq!(above[1 + i], buf[origin - STRIDE + i], "above[{i}]");
        assert_eq!(left[1 + i], buf[origin + (i + 0) * STRIDE - 1], "left[{i}]");
    }
}

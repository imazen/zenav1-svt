//! Issue #11 witness: loop restoration must walk the SAME restoration-unit
//! grid the search sized.
//!
//! The RU grid is counted off the TRUE (coded) frame extent — C runs
//! `svt_av1_alloc_restoration_struct` (which sets `horz_units_per_tile` /
//! `units_per_tile`) and every unit walk off ONE `whole_frame_rect(&cm->frm_size, ..)`,
//! and `cm->frm_size` is the pre-8-alignment size (`pcs.c:1337`,
//! `picture_width - non_m8_pad_w`). The port sized the grid on the true extent
//! but applied the filter over the ALIGNED canvas, so on a frame whose
//! alignment crosses a `count_units_in_tile` boundary the walk visited more
//! units than the grid holds and indexed out of bounds
//! (`restoration.rs:985`, "index out of bounds: the len is 2 but the index is 2").
//!
//! `383x512` is the smallest of the five renditions issue #11 reported and the
//! clearest instance: true 383 counts ONE horizontal unit ((383+128)/256 = 1)
//! while the aligned 384 walks TWO (256 + a 128-px remainder), so a 1x2 grid
//! got a 2x2 walk. `766x128` is the chroma twin — true chroma width
//! ceil(766/2) = 383 against an applied 768/2 = 384.
//!
//! Both tests carry an ANTI-VACUITY assertion that loop restoration actually
//! signalled Wiener on the plane in question. Without it the tests would keep
//! passing if a future change simply switched LR off at these sizes, which is
//! not the property being pinned.

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

const RESTORE_WIENER: u8 = 1;

/// The identity harness's `gradient` luma (`identity_run.rs`): a vertical ramp
/// XOR'd with a horizontal sawtooth. Enough residual texture at every block
/// size that the frame-level Wiener RD beats RESTORE_NONE.
fn gradient_y(w: usize, h: usize) -> Vec<u8> {
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            y[r * w + c] = (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8);
        }
    }
    y
}

/// Textured chroma — flat chroma codes to RESTORE_NONE on planes 1/2, which
/// would leave the chroma unit walk unexercised.
fn textured_uv(cw: usize, ch: usize, phase: usize) -> Vec<u8> {
    let mut p = vec![0u8; cw * ch];
    for r in 0..ch {
        for c in 0..cw {
            let ramp = ((r * 5 + c * 3 + phase) % 96) as i32;
            let tex = if (r / 3 + c / 3) % 2 == 0 { 22 } else { -18 };
            p[r * cw + c] = (96 + ramp + tex).clamp(16, 240) as u8;
        }
    }
    p
}

fn widen(s: &[u8]) -> Vec<u16> {
    s.iter().map(|&x| u16::from(x) << 2).collect()
}

fn pipeline(w: usize, h: usize, qp: u8, bd: u8) -> EncodePipeline {
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp,
        ..RcConfig::default()
    };
    EncodePipeline::new(w as u32, h as u32, 6, rc, 0, 1)
        .with_bit_depth(bd)
        .with_tile_rows_log2(0)
        .with_tile_cols_log2(0)
        .with_sb_size(None)
        .with_chroma_420(true)
}

/// The reported cell, in the reported shape: 10-bit 4:2:0, preset 6, native
/// u16 source through `try_encode_frame_420_hbd`. Pre-fix this panicked with
/// "index out of bounds: the len is 2 but the index is 2".
#[test]
fn issue11_bd10_420_383x512_luma_unit_walk_stays_in_the_grid() {
    let (w, h) = (383usize, 512usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let y = widen(&gradient_y(w, h));
    let u = widen(&textured_uv(cw, ch, 0));
    let v = widen(&textured_uv(cw, ch, 41));

    let mut p = pipeline(w, h, 40, 10);
    let bytes = p
        .try_encode_frame_420_hbd(&y, &u, &v, w)
        .expect("383x512 bd10 4:2:0 preset 6 must encode, not panic (issue #11)");
    assert!(!bytes.is_empty(), "encode produced no bytes");

    // Anti-vacuity: the luma plane must actually have signalled Wiener, i.e.
    // the walk that used to overrun really ran.
    assert_eq!(
        p.last_lr_stats.0[0], RESTORE_WIENER,
        "loop restoration did not fire on luma — this cell no longer exercises \
         the unit walk issue #11 was filed for; pick content/qp that does \
         rather than letting the test pass vacuously"
    );
}

/// Same defect on the CHROMA planes: true chroma width ceil(766/2) = 383 counts
/// one horizontal unit, the aligned 768/2 = 384 walks two.
#[test]
fn issue11_766x128_chroma_unit_walk_stays_in_the_grid() {
    let (w, h) = (766usize, 128usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let y = gradient_y(w, h);
    let u = textured_uv(cw, ch, 0);
    let v = textured_uv(cw, ch, 41);

    let mut p = pipeline(w, h, 40, 8);
    let bytes = p
        .try_encode_frame_420(&y, &u, &v, w)
        .expect("766x128 4:2:0 preset 6 must encode, not panic (issue #11)");
    assert!(!bytes.is_empty(), "encode produced no bytes");

    assert!(
        p.last_lr_stats.0[1] == RESTORE_WIENER || p.last_lr_stats.0[2] == RESTORE_WIENER,
        "loop restoration did not fire on either chroma plane — this cell no \
         longer exercises the chroma unit walk; frame types were {:?}",
        p.last_lr_stats.0
    );
}

/// The grid/walk agreement is a property of the frame geometry, not of one
/// rendition. Sweep non-8-aligned widths, three of which cross a
/// `count_units_in_tile(256, ..)` boundary once aligned and three of which do
/// not — the non-crossing ones are controls, they only ever had the wrong unit
/// EXTENTS, never an out-of-range index.
#[test]
fn issue11_alignment_crossing_widths_encode() {
    // true -> aligned, and whether the RU grid and the walk disagree:
    //   258 -> 264  no  (264 < 1.5*256, so the walk still takes one unit)
    //   383 -> 384  YES luma  (grid 1 col, walk 2)
    //   385 -> 392  no
    //   639 -> 640  YES luma  (grid 2 cols, walk 3)
    //   766 -> 768  YES chroma (true 383 -> 1 col, applied 384 -> 2)
    for &w in &[258usize, 383, 385, 639, 766] {
        let h = 128usize;
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let y = gradient_y(w, h);
        let u = textured_uv(cw, ch, 0);
        let v = textured_uv(cw, ch, 41);
        let mut p = pipeline(w, h, 40, 8);
        let bytes = p
            .try_encode_frame_420(&y, &u, &v, w)
            .unwrap_or_else(|e| panic!("{w}x{h} must encode: {e:?}"));
        assert!(!bytes.is_empty(), "{w}x{h} produced no bytes");
    }
}

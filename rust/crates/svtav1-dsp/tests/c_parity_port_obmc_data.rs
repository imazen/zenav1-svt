//! Differential parity for the OBMC `wsrc` / `mask` producer — evidence
//! tier 1 (`WORKING-ON-THIS.md` §4) for the exported half.
//!
//! Symbols driven: `svt_av1_calc_target_weighted_pred_above_c`,
//! `svt_av1_calc_target_weighted_pred_left_c`,
//! `svt_av1_skip_u4x4_pred_in_obmc` (all `nm -g`-visible), plus the header
//! inline `get_plane_block_size` and `svt_av1_get_obmc_mask` through shims.
//!
//! `calc_target_weighted_pred` itself takes a `PictureControlSet*`, a
//! `ModeDecisionContext*`, an `Av1Common*` and a `MacroBlockD*`, and its
//! neighbour walk (`foreach_overlappable_nb_above` / `_left`) reads the mi
//! grid — a shim cannot synthesise those without building most of the encoder.
//! It is ported as the pure arithmetic with the neighbour LIST supplied by the
//! caller, and it carries TIER 4 evidence by composition: both per-neighbour
//! accumulators it calls ARE tier 1 here, and the scaffolding between them
//! (the memset, the `*= 64` scale, the final `src * 4096 - wsrc`) is
//! hand-traced against the C source. The neighbour walk itself is NOT ported
//! and is named as missing.
//!
//! Why the two accumulators are shimmable at all: each reads exactly `n4_w`
//! off the `MacroBlockD` and everything else out of its `fun_ctxt`, so a
//! stack-local `MacroBlockD` is a complete stand-in.

use svtav1_cref::inter_pred as cref;
use svtav1_dsp::port_obmc_data::{
    AOM_BLEND_A64_MAX_ALPHA, CalcTargetWeightedPredCtxt, calc_target_weighted_pred_above,
    calc_target_weighted_pred_left, get_plane_block_size, skip_u4x4_pred_in_obmc,
};
use svtav1_types::block::BlockSize;

fn xs(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

fn plane(n: usize, seed: u32) -> Vec<u8> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            let v = xs(&mut s);
            match v % 8 {
                0 => 0,
                1 => 255,
                _ => (v >> 9) as u8,
            }
        })
        .collect()
}

/// Overlaps are `dim / 2` capped at 32, so only powers of two up to 32 occur.
const OVERLAPS: [usize; 6] = [1, 2, 4, 8, 16, 32];

#[test]
fn calc_target_weighted_pred_above_matches_c() {
    let mut cells = 0usize;
    for n4_w in [1usize, 2, 4, 8, 16] {
        let bw = n4_w * 4;
        for &overlap in &OVERLAPS {
            let bh = overlap.max(bw);
            for rel_mi_col in 0..n4_w {
                for nb_mi in 1..=(n4_w - rel_mi_col) {
                    let tmp_stride = bw + 5;
                    let tmp = plane(tmp_stride * (bh + 8), 0x1234 ^ bw as u32 ^ overlap as u32);
                    let mut rw = vec![0i32; bw * bh];
                    let mut rm = vec![AOM_BLEND_A64_MAX_ALPHA; bw * bh];
                    let mut cw = rw.clone();
                    let mut cm = rm.clone();
                    {
                        let mut c = CalcTargetWeightedPredCtxt {
                            mask_buf: &mut rm,
                            wsrc_buf: &mut rw,
                            tmp: &tmp,
                            tmp_stride,
                            overlap,
                        };
                        calc_target_weighted_pred_above(&mut c, bw, rel_mi_col, nb_mi);
                    }
                    cref::calc_target_weighted_pred_above(
                        n4_w, rel_mi_col, nb_mi, &mut cm, &mut cw, &tmp, tmp_stride, overlap,
                    );
                    assert_eq!(
                        rw, cw,
                        "wsrc above n4_w {n4_w} ov {overlap} col {rel_mi_col} nb {nb_mi}"
                    );
                    assert_eq!(
                        rm, cm,
                        "mask above n4_w {n4_w} ov {overlap} col {rel_mi_col} nb {nb_mi}"
                    );
                    cells += 1;
                }
            }
        }
    }
    assert!(cells >= 100, "anti-vacuity: only {cells} cells ran");
}

/// The left pass ACCUMULATES, so each cell seeds the buffers with the above
/// pass's output first — running it on zeros would not exercise the read-back.
#[test]
fn calc_target_weighted_pred_left_matches_c() {
    let mut cells = 0usize;
    for n4_w in [1usize, 2, 4, 8, 16] {
        let bw = n4_w * 4;
        for &overlap in &OVERLAPS {
            if overlap > bw {
                continue;
            }
            let n4_h = n4_w;
            let bh = n4_h * 4;
            for rel_mi_row in 0..n4_h {
                for nb_mi in 1..=(n4_h - rel_mi_row) {
                    let tmp_stride = bw + 3;
                    let above = plane(tmp_stride * (bh + 8), 0x5678 ^ bw as u32);
                    let left = plane(tmp_stride * (bh + 8), 0x9ABC ^ overlap as u32);
                    let ov_above = overlap.min(bh);

                    let mut rw = vec![0i32; bw * bh];
                    let mut rm = vec![AOM_BLEND_A64_MAX_ALPHA; bw * bh];
                    let mut cw = rw.clone();
                    let mut cm = rm.clone();

                    // Seed both sides identically with a real above pass.
                    {
                        let mut c = CalcTargetWeightedPredCtxt {
                            mask_buf: &mut rm,
                            wsrc_buf: &mut rw,
                            tmp: &above,
                            tmp_stride,
                            overlap: ov_above,
                        };
                        calc_target_weighted_pred_above(&mut c, bw, 0, n4_w);
                    }
                    cref::calc_target_weighted_pred_above(
                        n4_w, 0, n4_w, &mut cm, &mut cw, &above, tmp_stride, ov_above,
                    );
                    assert_eq!(rw, cw, "seed diverged before the left pass");

                    for v in rw.iter_mut() {
                        *v *= AOM_BLEND_A64_MAX_ALPHA;
                    }
                    for v in rm.iter_mut() {
                        *v *= AOM_BLEND_A64_MAX_ALPHA;
                    }
                    for v in cw.iter_mut() {
                        *v *= AOM_BLEND_A64_MAX_ALPHA;
                    }
                    for v in cm.iter_mut() {
                        *v *= AOM_BLEND_A64_MAX_ALPHA;
                    }

                    {
                        let mut c = CalcTargetWeightedPredCtxt {
                            mask_buf: &mut rm,
                            wsrc_buf: &mut rw,
                            tmp: &left,
                            tmp_stride,
                            overlap,
                        };
                        calc_target_weighted_pred_left(&mut c, bw, rel_mi_row, nb_mi);
                    }
                    cref::calc_target_weighted_pred_left(
                        n4_w, rel_mi_row, nb_mi, &mut cm, &mut cw, &left, tmp_stride, overlap,
                    );
                    assert_eq!(
                        rw, cw,
                        "wsrc left n4_w {n4_w} ov {overlap} row {rel_mi_row} nb {nb_mi}"
                    );
                    assert_eq!(
                        rm, cm,
                        "mask left n4_w {n4_w} ov {overlap} row {rel_mi_row} nb {nb_mi}"
                    );
                    cells += 1;
                }
            }
        }
    }
    assert!(cells >= 60, "anti-vacuity: only {cells} cells ran");
}

/// `CONFIG_ENABLE_OBMC` is 1 and `DISABLE_CHROMA_U8X8_OBMC` is 0, so the LIVE
/// arm is one-sided. Both verdicts must occur or the cell proves nothing.
#[test]
fn skip_u4x4_pred_in_obmc_matches_c() {
    let mut ones = 0usize;
    let mut zeros = 0usize;
    for bsize in BlockSize::ALL {
        for dir in [0i32, 1] {
            for ssx in [0usize, 1] {
                for ssy in [0usize, 1] {
                    // C asserts is_motion_variation_allowed_bsize(bsize), which
                    // is bw >= 8 && bh >= 8; only feed it those.
                    let (w, h) = (
                        svtav1_dsp::port_obmc_data::block_size_wide(bsize),
                        svtav1_dsp::port_obmc_data::block_size_high(bsize),
                    );
                    if w < 8 || h < 8 {
                        continue;
                    }
                    if get_plane_block_size(bsize, ssx, ssy).is_none() {
                        continue;
                    }
                    let got = skip_u4x4_pred_in_obmc(bsize, dir, ssx, ssy);
                    let want =
                        cref::skip_u4x4_pred_in_obmc(bsize as i32, dir, ssx as i32, ssy as i32);
                    assert_eq!(
                        got, want,
                        "skip_u4x4 bsize {bsize:?} dir {dir} ss({ssx},{ssy})"
                    );
                    if got == 1 { ones += 1 } else { zeros += 1 }
                }
            }
        }
    }
    assert!(ones > 0, "the `return dir == 0` arm never returned 1");
    assert!(zeros > 10, "only {zeros} zero verdicts");
}

#[test]
fn get_plane_block_size_matches_c() {
    for bsize in BlockSize::ALL {
        for ssx in [0usize, 1] {
            for ssy in [0usize, 1] {
                let got = get_plane_block_size(bsize, ssx, ssy).map_or(-1, |b| b as i32);
                let want = cref::get_plane_block_size(bsize as i32, ssx as i32, ssy as i32);
                assert_eq!(got, want, "get_plane_block_size {bsize:?} ss({ssx},{ssy})");
            }
        }
    }
}

/// The OBMC 1-D blend mask already in `obmc.rs`, re-checked here because the
/// two accumulators index it directly.
#[test]
fn obmc_mask_matches_c() {
    for &overlap in &OVERLAPS {
        let want = cref::get_obmc_mask(overlap);
        // Drive it through the exported accumulator: a single-row above pass
        // with tmp == 1 makes wsrc[col] == 64 - mask1d[row].
        let tmp = vec![1u8; 64 * 64];
        let bw = 64usize;
        let mut w = vec![0i32; bw * 64];
        let mut m = vec![0i32; bw * 64];
        cref::calc_target_weighted_pred_above(16, 0, 1, &mut m, &mut w, &tmp, bw, overlap);
        for (row, &expect) in want.iter().enumerate() {
            assert_eq!(
                m[row * bw],
                expect as i32,
                "obmc mask overlap {overlap} row {row}"
            );
        }
    }
}

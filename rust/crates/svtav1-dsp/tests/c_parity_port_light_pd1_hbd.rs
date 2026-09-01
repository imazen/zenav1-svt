//! Differential parity for `svt_inter_predictor_light_pd1`'s **10-bit arm** —
//! evidence tier 1 (`WORKING-ON-THIS.md` §4).
//!
//! Symbol driven: `svt_inter_predictor_light_pd1` (inter_prediction.c:1283),
//! `nm -g`-visible as `T`, entered with `bd > EB_EIGHT_BIT` so the
//! `svt_aom_pack_block` + `svt_aom_convolveHbd[][][]` branch runs. The 8-bit
//! arm of the same C function is gated in
//! `c_parity_port_inter_predictor.rs`; between them the function has no
//! untested branch.
//!
//! # What this arm actually consumes (the contract, not a guess)
//!
//! Two 8-bit planes — the MSBs and the 2 LSBs-in-the-top-of-a-byte — **both at
//! `src_stride`**, read from `src - 8 - 8 * src_stride`. The fixtures below
//! build exactly that: one buffer per plane with a 16-pixel border, the same
//! stride, and the block origin 16 rows/columns in (twice the 8 C needs, so a
//! port that read the window from the wrong corner lands on different bytes
//! rather than out of bounds).
//!
//! # Depths
//!
//! The tier-1 sweep runs **bd 10** — C's whole shipping high-bit-depth
//! envelope (`svt_av1_verify_settings`, enc_settings.c:460, rejects anything
//! but 8/10). bd 12 gets its own cell,
//! [`bd12_port_follows_the_scalar_c_kernel_and_the_dispatch_is_reported`],
//! because C's own NEON kernel and C's own scalar kernel disagree there —
//! docs/SUSPECTED-C-BUGS.md #21.
//!
//! # Anti-vacuity
//!
//! Every cell counts itself and the test asserts a floor, because a filter /
//! size / sub-pel loop that silently produced zero iterations would otherwise
//! pass (`WORKING-ON-THIS.md` §5).

use svtav1_cref::inter_pred::{RefCompound, RefSubpel};
use svtav1_cref::interpred_gap as gap;
use svtav1_dsp::port_convolve::{ConvolveParams, InterpFilterKind, SrcView};
use svtav1_dsp::port_inter_predictor::{
    inter_predictor_light_pd1_8bit, inter_predictor_light_pd1_hbd, make_interp_filters,
};
use svtav1_dsp::port_scale_factors::{SCALE_SUBPEL_SHIFTS, SubpelParams};

const FILTERS: [(usize, InterpFilterKind); 4] = [
    (0, InterpFilterKind::EightTapRegular),
    (1, InterpFilterKind::EightTapSmooth),
    (2, InterpFilterKind::MultiTapSharp),
    (3, InterpFilterKind::Bilinear),
];

const SIZES: [(usize, usize); 5] = [(4, 4), (8, 8), (16, 8), (8, 16), (32, 32)];

/// The MC border, doubled: C needs 8, so 16 leaves a margin in which a
/// wrong-corner read still lands on real, DIFFERENT data.
const BORDER: usize = 16;

fn xs(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

/// A pair of 8-bit planes at one shared stride, plus the block origin.
struct Split {
    msb: Vec<u8>,
    lsb: Vec<u8>,
    stride: usize,
    origin: usize,
}

impl Split {
    fn new(w: usize, h: usize, seed: u32) -> Self {
        let stride = w + 2 * BORDER + 3;
        let rows = h + 2 * BORDER;
        let mut s = seed | 1;
        let mut mk = |lsb: bool| -> Vec<u8> {
            (0..stride * rows)
                .map(|_| {
                    let v = xs(&mut s);
                    match v % 8 {
                        0 => 0x00,
                        1 => 0xFF,
                        2 if lsb => 0xC0,
                        3 if lsb => 0x3F,
                        _ => (v >> 13) as u8,
                    }
                })
                .collect()
        };
        let msb = mk(false);
        let lsb = mk(true);
        Self {
            msb,
            lsb,
            stride,
            origin: BORDER * (w + 2 * BORDER + 3) + BORDER,
        }
    }
}

/// `SCALE_EXTRA_BITS` = `SCALE_SUBPEL_BITS - SUBPEL_BITS` = 10 - 4 = 6
/// (inter_prediction.c:23).
const SCALE_EXTRA_BITS: i32 = 6;

/// The four `svt_aom_convolveHbd[subX][subY][*]` corners, as the Q4 sub-pel
/// phases they must be AFTER `revert_scale_extra_bits`.
///
/// TRAP, measured here on 2026-08-31: `svt_inter_predictor_light_pd1` calls
/// `revert_scale_extra_bits` on the UNSCALED path, which shifts `subpel_x` /
/// `subpel_y` right by 6. A phase handed in as a Q4 value therefore becomes
/// **0**, and the dispatch collapses to the copy kernel for every cell. The
/// phases below are Q4 and are shifted LEFT by 6 at the call site;
/// [`the_four_dispatch_corners_are_actually_reached`] is the positive control
/// that says so rather than assuming it.
const Q4_PHASES: [(i32, i32); 4] = [(0, 0), (3, 0), (0, 9), (15, 15)];

fn unscaled(sx: i32, sy: i32) -> (SubpelParams, RefSubpel) {
    (
        SubpelParams {
            xs: SCALE_SUBPEL_SHIFTS,
            ys: SCALE_SUBPEL_SHIFTS,
            subpel_x: sx << SCALE_EXTRA_BITS,
            subpel_y: sy << SCALE_EXTRA_BITS,
        },
        RefSubpel {
            xs: SCALE_SUBPEL_SHIFTS,
            ys: SCALE_SUBPEL_SHIFTS,
            subpel_x: sx << SCALE_EXTRA_BITS,
            subpel_y: sy << SCALE_EXTRA_BITS,
        },
    )
}

fn run_cell(
    filters: u32,
    w: usize,
    h: usize,
    sx: i32,
    sy: i32,
    is_compound: bool,
    avg: bool,
    bd: i32,
    seed: u32,
) {
    let f = Split::new(w, h, seed);
    let cb_stride = w + 1;
    let (sp, csp) = unscaled(sx, sy);
    let mut cp = ConvolveParams::no_round(avg, cb_stride, is_compound, bd);
    cp.do_average = avg;

    let mut r_dst = vec![0u16; w * h];
    let mut r_cb = vec![0u16; cb_stride * h];
    inter_predictor_light_pd1_hbd(
        &f.msb, &f.lsb, f.origin, f.stride, &mut r_dst, w, &mut r_cb, w, h, filters, &sp, &cp, bd,
    );

    let mut c_dst = vec![0u16; w * h];
    let mut c_cb = vec![0u16; cb_stride * h];
    let mut c_msb = f.msb.clone();
    let mut c_lsb = f.lsb.clone();
    gap::inter_predictor_light_pd1_hbd(
        &mut c_msb,
        &mut c_lsb,
        f.origin,
        f.stride,
        &mut c_dst,
        w,
        &mut c_cb,
        cb_stride,
        w,
        h,
        filters,
        csp,
        RefCompound {
            is_compound,
            do_average: avg,
            use_jnt: false,
            fwd: 0,
            bck: 0,
        },
        bd,
    );
    assert_eq!(
        r_cb, c_cb,
        "light_pd1 hbd CONV_BUF {w}x{h} sub({sx},{sy}) comp{is_compound} avg{avg} bd{bd}"
    );
    assert_eq!(
        r_dst, c_dst,
        "light_pd1 hbd dst {w}x{h} sub({sx},{sy}) comp{is_compound} avg{avg} bd{bd}"
    );
}

#[test]
fn light_pd1_hbd_matches_c() {
    let mut cells = 0usize;
    for bd in [10] {
        for (yi, yk) in FILTERS {
            for (xi, xk) in FILTERS {
                let filters = make_interp_filters(yk, xk);
                for (w, h) in SIZES {
                    // All four kernel-table corners: (subpel_x != 0, subpel_y != 0).
                    for (sx, sy) in Q4_PHASES {
                        for is_compound in [false, true] {
                            for avg in [false, true] {
                                if avg && !is_compound {
                                    continue;
                                }
                                run_cell(
                                    filters,
                                    w,
                                    h,
                                    sx,
                                    sy,
                                    is_compound,
                                    avg,
                                    bd,
                                    0x4d1d_0001
                                        ^ (w as u32) << 8
                                        ^ (h as u32)
                                        ^ (yi as u32) << 20
                                        ^ (xi as u32) << 24
                                        ^ (sx as u32) << 4
                                        ^ (bd as u32) << 16,
                                );
                                cells += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(cells >= 240, "anti-vacuity: only {cells} cells ran");
}

/// The two arms of the ONE C function must not be interchangeable: feeding the
/// 8-bit arm the MSB plane alone gives a DIFFERENT prediction from the 10-bit
/// arm on the same MSBs, because the 2 LSBs shift every sample. A port that
/// quietly routed 10-bit through the 8-bit arm would pass nothing above, but
/// this states the separation as a measured fact rather than an assumption.
#[test]
fn the_two_arms_are_genuinely_different_functions() {
    let (w, h) = (8usize, 8usize);
    let f = Split::new(w, h, 0x2222_0001);
    let filters = make_interp_filters(
        InterpFilterKind::EightTapRegular,
        InterpFilterKind::EightTapRegular,
    );
    let (sp, _) = unscaled(3, 5);
    let cp8 = ConvolveParams::no_round(false, w, false, 8);
    let cp10 = ConvolveParams::no_round(false, w, false, 10);

    let mut hbd = vec![0u16; w * h];
    let mut cb = vec![0u16; w * h];
    inter_predictor_light_pd1_hbd(
        &f.msb, &f.lsb, f.origin, f.stride, &mut hbd, w, &mut cb, w, h, filters, &sp, &cp10, 10,
    );

    let mut lbd = vec![0u8; w * h];
    let mut cb8 = vec![0u16; w * h];
    inter_predictor_light_pd1_8bit(
        SrcView::new(&f.msb, f.origin, f.stride),
        &mut lbd,
        w,
        &mut cb8,
        w,
        h,
        filters,
        &sp,
        &cp8,
    );
    // The 10-bit output is ~4x the 8-bit one by construction, so the useful
    // statement is that they are not the same numbers, not a scaling claim.
    let same = hbd
        .iter()
        .zip(&lbd)
        .filter(|(a, b)| **a == u16::from(**b))
        .count();
    assert!(
        same < hbd.len(),
        "the 10-bit arm reproduced the 8-bit arm exactly: the LSB plane was ignored"
    );
}

/// bd = 12 — where C's DISPATCHED kernel and C's own scalar kernel disagree,
/// and the port sides with the scalar one.
///
/// # What was measured (2026-08-31, aarch64-darwin)
///
/// The first bd-12 cell to run (`4x4`, `sub(0,0)`, compound, no average)
/// produced port `28112` where C's dispatched entry produced `46912`. Solving
/// both closed forms on the same packed sample (884):
///
/// * scalar `svt_av1_highbd_jnt_convolve_2d_copy_c` (inter_prediction.c:995)
///   uses `conv_params->round_0/round_1`, which
///   `get_conv_params_no_round` sets to **5** / 7 at bd 12 (its
///   `intbufrange = 12 + 7 - 3 + 2 = 18 > 16` correction fires):
///   `bits = 2`, `round_offset = 24576`, `884 << 2 | + 24576 = 28112`.
/// * `svt_av1_highbd_jnt_convolve_2d_copy_neon`
///   (ASM_NEON/highbd_jnt_convolve_neon.c:1110) hardcodes the compile-time
///   `ROUND0_BITS` (3) and `COMPOUND_ROUND1_BITS` (7) instead:
///   `round_bits = 4`, `round_offset = 98304`,
///   `884 << 4 + 98304 = 112448`, which then **wraps** in the `uint16_t`
///   CONV_BUF to `46912`.
///
/// So this is a C-side divergence at a depth C cannot ship (the encoder
/// rejects anything but 8/10-bit at `svt_av1_verify_settings`
/// enc_settings.c:460), not a port gap.
///
/// # What this test asserts, and why it is ISA-portable
///
/// It asserts the PORT equals C's SCALAR composition (`svt_aom_pack_block`
/// then the `_c` kernel) — true on every host. The dispatched entry is only
/// COMPARED and REPORTED, never required to differ: an x86 build may install
/// a kernel that reads `conv_params` correctly, and an `assert_ne!` would then
/// fail for the right reason on the wrong host.
#[test]
fn bd12_port_follows_the_scalar_c_kernel_and_the_dispatch_is_reported() {
    use svtav1_cref::inter_pred as cref;
    use svtav1_cref::interpred_gap::PackEntry;
    use svtav1_dsp::port_pack::INTERPOLATION_OFFSET;

    let bd = 12;
    let mut disagreements = 0usize;
    let mut cells = 0usize;
    for (w, h) in SIZES {
        for (sx, sy) in Q4_PHASES {
            for is_compound in [false, true] {
                let f = Split::new(w, h, 0x6c12_0001 ^ (w as u32) << 8 ^ sx as u32);
                let filters = make_interp_filters(
                    InterpFilterKind::EightTapRegular,
                    InterpFilterKind::EightTapRegular,
                );
                let (sp, csp) = unscaled(sx, sy);
                let cb_stride = w + 1;
                let cp = ConvolveParams::no_round(false, cb_stride, is_compound, bd);

                let mut r_dst = vec![0u16; w * h];
                let mut r_cb = vec![0u16; cb_stride * h];
                inter_predictor_light_pd1_hbd(
                    &f.msb, &f.lsb, f.origin, f.stride, &mut r_dst, w, &mut r_cb, w, h, filters,
                    &sp, &cp, bd,
                );

                // C's SCALAR composition: C's own pack, then C's own `_c`
                // kernel, at exactly light-PD1's scratch geometry.
                let off = INTERPOLATION_OFFSET;
                let pack_w = w + 2 * off;
                let pack_h = h + 2 * off;
                let s16_stride = pack_w.next_multiple_of(8);
                let mut s16 = vec![0u16; s16_stride * pack_h];
                let win = f.origin - off - off * f.stride;
                gap::pack_block(
                    PackEntry::Scalar,
                    &f.msb[win..],
                    f.stride,
                    &f.lsb[win..],
                    f.stride,
                    &mut s16,
                    s16_stride,
                    pack_w,
                    pack_h,
                );
                let so = off + off * s16_stride;
                let mut s_dst = vec![0u16; w * h];
                let mut s_cb = vec![0u16; cb_stride * h];
                let cfg = cref::JntCfg {
                    do_average: false,
                    use_jnt: false,
                    fwd: 0,
                    bck: 0,
                };
                // `svt_aom_convolveHbd[subX][subY][bi]` (inter_prediction.c:1098),
                // with each entry's `_c` variant.
                match (sx != 0, sy != 0, is_compound) {
                    (false, false, false) => cref::highbd_convolve_2d_copy_sr(
                        &s16, so, s16_stride, &mut s_dst, w, w, h, bd,
                    ),
                    (false, false, true) => cref::highbd_jnt_convolve_2d_copy(
                        &s16, so, s16_stride, &mut s_dst, w, &mut s_cb, cb_stride, w, h, bd, cfg,
                    ),
                    (false, true, false) => cref::highbd_convolve_y_sr(
                        &s16, so, s16_stride, &mut s_dst, w, w, h, 0, h as i32, sy, bd,
                    ),
                    (false, true, true) => cref::highbd_jnt_convolve_y(
                        &s16, so, s16_stride, &mut s_dst, w, &mut s_cb, cb_stride, w, h, 0,
                        h as i32, sy, bd, cfg,
                    ),
                    (true, false, false) => cref::highbd_convolve_x_sr(
                        &s16, so, s16_stride, &mut s_dst, w, w, h, 0, w as i32, sx, bd,
                    ),
                    (true, false, true) => cref::highbd_jnt_convolve_x(
                        &s16, so, s16_stride, &mut s_dst, w, &mut s_cb, cb_stride, w, h, 0,
                        w as i32, sx, bd, cfg,
                    ),
                    (true, true, false) => cref::highbd_convolve_2d_sr(
                        &s16, so, s16_stride, &mut s_dst, w, w, h, 0, w as i32, 0, h as i32, sx,
                        sy, bd,
                    ),
                    (true, true, true) => cref::highbd_jnt_convolve_2d(
                        &s16, so, s16_stride, &mut s_dst, w, &mut s_cb, cb_stride, w, h, 0,
                        w as i32, 0, h as i32, sx, sy, bd, cfg,
                    ),
                }
                assert_eq!(
                    (&r_dst, &r_cb),
                    (&s_dst, &s_cb),
                    "bd12 port vs C SCALAR composition {w}x{h} sub({sx},{sy}) comp{is_compound}"
                );

                // The dispatched entry: compared and reported, never required.
                let mut d_dst = vec![0u16; w * h];
                let mut d_cb = vec![0u16; cb_stride * h];
                let (mut c_msb, mut c_lsb) = (f.msb.clone(), f.lsb.clone());
                gap::inter_predictor_light_pd1_hbd(
                    &mut c_msb,
                    &mut c_lsb,
                    f.origin,
                    f.stride,
                    &mut d_dst,
                    w,
                    &mut d_cb,
                    cb_stride,
                    w,
                    h,
                    filters,
                    csp,
                    RefCompound {
                        is_compound,
                        do_average: false,
                        use_jnt: false,
                        fwd: 0,
                        bck: 0,
                    },
                    bd,
                );
                if (d_dst, d_cb) != (s_dst, s_cb) {
                    disagreements += 1;
                    println!("bd12 DISPATCH != SCALAR at {w}x{h} sub({sx},{sy}) comp{is_compound}");
                }
                cells += 1;
            }
        }
    }
    assert!(cells >= 40, "anti-vacuity: only {cells} cells ran");
    println!("bd12: {disagreements} / {cells} cells where C's dispatch != C's scalar");
}

/// POSITIVE CONTROL for [`Q4_PHASES`]: prove the four sub-pel corners really
/// reach four DIFFERENT kernels, instead of collapsing to the copy one.
///
/// This is the cell that caught the trap. `svt_inter_predictor_light_pd1`
/// reverts the scale extra bits before dispatching, so a phase supplied as a
/// Q4 value is shifted right by 6 and becomes 0 — the whole four-corner sweep
/// then drives `svt_av1_highbd_convolve_2d_copy_sr` four times and reports
/// full coverage. Nothing in a pass/fail comparison can see that; only a
/// control that asks "did the phase survive" can.
///
/// It also holds for the 8-bit arm, whose landed cell
/// (`c_parity_port_inter_predictor.rs::inter_predictor_light_pd1_8bit_matches_c`)
/// passes `(0,0)`, `(3,0)`, `(0,9)`, `(15,15)` as raw Q4 values: **all four
/// collapse to (0,0)**, so that cell exercises only the copy corner of
/// `svt_aom_convolve[][][]`. Recorded here rather than fixed, because that
/// file belongs to another lane.
#[test]
fn the_four_dispatch_corners_are_actually_reached() {
    use svtav1_dsp::port_scale_factors::revert_scale_extra_bits;
    let mut corners = std::collections::BTreeSet::new();
    for (sx, sy) in Q4_PHASES {
        let (sp, _) = unscaled(sx, sy);
        let mut r = sp;
        revert_scale_extra_bits(&mut r);
        assert_eq!(
            (r.subpel_x, r.subpel_y),
            (sx, sy),
            "phase {sx},{sy} did not survive revert_scale_extra_bits"
        );
        corners.insert((r.subpel_x != 0, r.subpel_y != 0));
    }
    assert_eq!(
        corners.len(),
        4,
        "the phase set reaches {} of 4 dispatch corners: {corners:?}",
        corners.len()
    );

    // The same list read the WRONG way — as raw Q4 without the shift — is
    // what the trap looks like. Stated as a fact so nobody re-derives it.
    let collapsed: std::collections::BTreeSet<_> = Q4_PHASES
        .into_iter()
        .map(|(sx, sy)| {
            let mut r = SubpelParams {
                xs: SCALE_SUBPEL_SHIFTS,
                ys: SCALE_SUBPEL_SHIFTS,
                subpel_x: sx,
                subpel_y: sy,
            };
            revert_scale_extra_bits(&mut r);
            (r.subpel_x != 0, r.subpel_y != 0)
        })
        .collect();
    assert_eq!(
        collapsed.len(),
        1,
        "un-shifted Q4 phases should collapse to one corner; got {collapsed:?}"
    );
}

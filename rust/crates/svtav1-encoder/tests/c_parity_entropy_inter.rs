//! Differential tests for the inter bitstream-syntax port
//! (`svtav1_encoder::port_entropy_inter`) against the REAL exported C
//! symbols of `Source/Lib/Codec/entropy_coding.c`.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4) for everything here:
//! every assertion's right-hand side comes from `svtav1-cref`'s
//! `entropy_inter` shim, which calls the C library's own exported functions
//! on the same inputs. Nothing in this file compares one transcription
//! against another.
//!
//! The `static` C functions of the same group (`write_ref_frames`,
//! `write_inter_mode`, `write_drl_idx`, `write_motion_mode`,
//! `write_mb_interp_filter`, `write_global_motion*`, the `aom_wb_write_*`
//! primitives below `svt_aom_wb_write_signed_primitive_refsubexpfin`,
//! `write_sgrproj_filter`) are defined in `entropy_coding.c`, which
//! `shims/ref_shims.c` never compiles — no shim can reach them, so tier 1 is
//! structurally unavailable for those. They are covered here only through
//! their tier-1-gated inputs (contexts, CDF selectors, default tables) plus
//! the one exported primitive.

use svtav1_cref::entropy_inter as cref;
use svtav1_encoder::port_entropy_inter as p;
use svtav1_encoder::port_entropy_inter::modes::{MotionMode, TransformationType};
use svtav1_types::block::BlockSize;

// ---- neighbour grid the sweeps walk ----

/// A representative spread of neighbour states: intra, IntraBC, every single
/// reference, and unidirectional / bidirectional compound pairs.
fn neighbor_cases() -> Vec<(p::NeighborMi, cref::NeighborDesc)> {
    let mut out = Vec::new();
    let modes: [u8; 4] = [
        0,  /*DC_PRED*/
        13, /*NEARESTMV*/
        16, /*NEWMV*/
        24, /*NEW_NEWMV*/
    ];
    // (ref0, ref1): intra, IntraBC-shaped, all 7 singles, 4 unidir and 4
    // bidir compound pairs.
    let pairs: [(i8, i8); 20] = [
        (0, -1),
        (1, -1),
        (2, -1),
        (3, -1),
        (4, -1),
        (5, -1),
        (6, -1),
        (7, -1),
        (1, 2),
        (1, 3),
        (1, 4),
        (5, 7),
        (1, 5),
        (1, 6),
        (1, 7),
        (4, 5),
        (3, 7),
        (2, 6),
        (6, 7),
        (5, 6),
    ];
    for (mi, mode) in modes.iter().enumerate() {
        for (pi, (r0, r1)) in pairs.iter().enumerate() {
            for &valid in &[true, false] {
                for &ibc in &[false, true] {
                    let bsize = ((mi * 7 + pi) % 22) as u8;
                    let mi_p = p::NeighborMi {
                        mode: *mode,
                        ref_frame: [*r0, *r1],
                        interp_filters: ((pi as u32 % 3) << 16) | (mi as u32 % 3),
                        use_intrabc: ibc,
                        skip_mode: (pi % 2) == 1,
                        comp_group_idx: (pi % 2) as u8,
                        compound_idx: (mi % 2) as u8,
                        bsize,
                    };
                    let mi_c = cref::NeighborDesc {
                        valid,
                        mode: *mode as i32,
                        ref_frame: [*r0 as i32, *r1 as i32],
                        interp_filters: mi_p.interp_filters,
                        use_intrabc: ibc,
                        skip_mode: mi_p.skip_mode,
                        comp_group_idx: mi_p.comp_group_idx,
                        compound_idx: mi_p.compound_idx,
                        bsize: bsize as i32,
                    };
                    out.push((
                        if valid {
                            mi_p
                        } else {
                            p::NeighborMi::default()
                        },
                        mi_c,
                    ));
                    if !valid {
                        // A null pointer carries no fields; one entry is enough.
                        break;
                    }
                }
            }
        }
    }
    out
}

/// Build the port-side `Neighbors` and the shim-side pair for one cell.
fn build(
    a: (p::NeighborMi, cref::NeighborDesc),
    l: (p::NeighborMi, cref::NeighborDesc),
    up: bool,
    left: bool,
) -> (p::Neighbors, cref::NeighborDesc, cref::NeighborDesc) {
    let nb = p::Neighbors {
        above: a.1.valid.then_some(a.0),
        left: l.1.valid.then_some(l.0),
        up_available: up,
        left_available: left,
    };
    (nb, a.1, l.1)
}

/// Every (above, left, up_avail, left_avail) cell the sweeps below share.
///
/// C dereferences `above_mbmi` whenever `up_available` is set, so a cell with
/// `up_available && !valid` would be a null dereference in C, not a
/// meaningful input; those cells are skipped rather than compared.
fn cells() -> Vec<(
    p::Neighbors,
    cref::NeighborDesc,
    cref::NeighborDesc,
    bool,
    bool,
)> {
    let cases = neighbor_cases();
    let mut out = Vec::new();
    for (i, a) in cases.iter().enumerate() {
        let l = cases[(i * 7 + 3) % cases.len()];
        for &(up, left) in &[(true, true), (true, false), (false, true), (false, false)] {
            if (up && !a.1.valid) || (left && !l.1.valid) {
                continue;
            }
            let (nb, ad, ld) = build(*a, l, up, left);
            out.push((nb, ad, ld, up, left));
        }
    }
    assert!(out.len() > 400, "sweep is only {} cells", out.len());
    out
}

// ---- 1. neighbour ref counts ----

#[test]
fn collect_neighbors_ref_counts_matches_c() {
    let mut checked = 0usize;
    let mut nonzero = 0usize;
    for (nb, ad, ld, up, left) in cells() {
        let got = p::refframe::collect_neighbors_ref_counts(&nb);
        let want = cref::collect_neighbors_ref_counts(ad, ld, up, left);
        assert_eq!(
            got, want,
            "ref counts differ (up={up} left={left}, above={ad:?}, left={ld:?})"
        );
        if want.iter().any(|&c| c != 0) {
            nonzero += 1;
        }
        checked += 1;
    }
    // Anti-vacuity: a probe that only ever sees all-zero counts proves
    // nothing about the counting itself.
    assert!(checked > 400, "only {checked} cells");
    assert!(
        nonzero > 100,
        "only {nonzero} cells produced a nonzero count"
    );
}

// ---- 2. every prediction context ----

#[test]
fn ref_frame_contexts_match_c() {
    let mut seen: [std::collections::BTreeSet<i32>; cref::N_CTX] =
        std::array::from_fn(|_| std::collections::BTreeSet::new());
    for (nb, ad, ld, up, left) in cells() {
        let counts = p::refframe::collect_neighbors_ref_counts(&nb);
        let want = cref::ref_contexts(ad, ld, up, left);
        let got: [i32; cref::N_CTX] = [
            p::refframe::pred_context_single_ref_p1(&counts) as i32,
            p::refframe::pred_context_single_ref_p2(&counts) as i32,
            p::refframe::pred_context_single_ref_p3(&counts) as i32,
            p::refframe::pred_context_single_ref_p4(&counts) as i32,
            p::refframe::pred_context_single_ref_p5(&counts) as i32,
            p::refframe::pred_context_single_ref_p6(&counts) as i32,
            p::refframe::pred_context_comp_ref_p(&counts) as i32,
            p::refframe::pred_context_comp_ref_p1(&counts) as i32,
            p::refframe::pred_context_comp_ref_p2(&counts) as i32,
            p::refframe::pred_context_comp_bwdref_p(&counts) as i32,
            p::refframe::pred_context_comp_bwdref_p1(&counts) as i32,
            p::refframe::pred_context_uni_comp_ref_p(&counts) as i32,
            p::refframe::pred_context_uni_comp_ref_p1(&counts) as i32,
            p::refframe::pred_context_uni_comp_ref_p2(&counts) as i32,
            p::refframe::reference_mode_context(&nb) as i32,
            p::refframe::comp_reference_type_context(&nb) as i32,
            p::intra_inter_context(&nb) as i32,
            p::modes::skip_mode_context(&nb) as i32,
            p::modes::comp_group_idx_context(&nb) as i32,
        ];
        assert_eq!(
            got, want,
            "context vector differs (up={up} left={left}, above={ad:?}, left={ld:?})"
        );
        for (s, v) in seen.iter_mut().zip(want.iter()) {
            s.insert(*v);
        }
    }
    // Anti-vacuity: every context must actually vary over the sweep, or the
    // agreement is just "both returned the same constant".
    for (i, s) in seen.iter().enumerate() {
        assert!(s.len() >= 2, "context slot {i} never varied: {s:?}");
    }
}

// ---- 3. the CDF selectors, as flat row indices ----

#[test]
fn cdf_selector_rows_match_c() {
    for (nb, ad, ld, up, left) in cells() {
        let counts = p::refframe::collect_neighbors_ref_counts(&nb);
        let want = cref::cdf_rows(ad, ld, up, left);
        // C's selectors return a pointer into `[ctx][slot]`; the port returns
        // the pair, so the flat row is ctx * <slots> + slot.
        let sr = |n: usize| {
            let (c, s) = p::refframe::pred_cdf_single_ref(&counts, n);
            (c * 6 + s) as i32
        };
        let cr = |n: usize| {
            let (c, s) = p::refframe::pred_cdf_comp_ref(&counts, n);
            (c * 3 + s) as i32
        };
        let cb = |n: usize| {
            let (c, s) = p::refframe::pred_cdf_comp_bwdref(&counts, n);
            (c * 2 + s) as i32
        };
        let uc = |n: usize| {
            let (c, s) = p::refframe::pred_cdf_uni_comp_ref(&counts, n);
            (c * 3 + s) as i32
        };
        let got: [i32; cref::N_CDF_ROWS] = [
            sr(1),
            sr(2),
            sr(3),
            sr(4),
            sr(5),
            sr(6),
            cr(0),
            cr(1),
            cr(2),
            cb(0),
            cb(1),
            uc(0),
            uc(1),
            uc(2),
            p::refframe::reference_mode_context(&nb) as i32,
            p::refframe::comp_reference_type_context(&nb) as i32,
        ];
        assert_eq!(
            got, want,
            "CDF rows differ (up={up} left={left}, above={ad:?}, left={ld:?})"
        );
    }
}

// ---- 4. compound index context ----

#[test]
fn comp_index_context_matches_c() {
    let cases = neighbor_cases();
    let mut seen = std::collections::BTreeSet::new();
    let mut checked = 0usize;
    for (i, a) in cases.iter().enumerate().step_by(3) {
        let l = cases[(i * 5 + 1) % cases.len()];
        let nb = p::Neighbors {
            above: a.1.valid.then_some(a.0),
            left: l.1.valid.then_some(l.0),
            up_available: true,
            left_available: true,
        };
        for &eoh in &[true, false] {
            for &bits in &[3u32, 7] {
                // The first five are symmetric (fwd == bck, so `offset` is
                // 1); the last four are deliberately asymmetric so the
                // `offset == 0` half of the context space is reached too.
                for &(cur, bck, fwd) in &[
                    (4, 0, 8),
                    (4, 2, 6),
                    (0, 0, 0),
                    (10, 9, 11),
                    (5, 1, 9),
                    (4, 3, 8),
                    (6, 5, 2),
                    (7, 0, 1),
                    (2, 1, 6),
                ] {
                    let got = p::modes::comp_index_context(eoh, bits, cur, bck, fwd, &nb) as i32;
                    let want = cref::comp_index_context(eoh, bits as i32, cur, bck, fwd, a.1, l.1);
                    assert_eq!(
                        got, want,
                        "comp_index ctx (eoh={eoh} bits={bits} {cur}/{bck}/{fwd})"
                    );
                    seen.insert(want);
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 200, "only {checked} cells");
    assert!(
        seen.len() >= 5,
        "context never varied enough: {seen:?} — the sweep must reach both the\n         offset==0 and offset==1 halves"
    );
}

// ---- 5. switchable interpolation-filter context ----

#[test]
fn switchable_interp_context_matches_c() {
    let mut seen = std::collections::BTreeSet::new();
    for (nb, ad, ld, up, left) in cells().into_iter().step_by(3) {
        for rf0 in [1i8, 4, 5, 7] {
            for rf1 in [-1i8, 0, 5] {
                for dir in 0..2 {
                    let got = p::interp::pred_context_switchable_interp(rf0, rf1, &nb, dir) as i32;
                    let want = cref::switchable_interp_context(
                        rf0 as i32, rf1 as i32, dir, ad, ld, up, left,
                    );
                    assert_eq!(
                        got, want,
                        "switchable interp ctx (rf0={rf0} rf1={rf1} dir={dir} up={up} left={left})"
                    );
                    seen.insert(want);
                }
            }
        }
    }
    assert!(seen.len() >= 8, "context never varied enough: {seen:?}");
}

// ---- 6. non-translational global motion, interintra, motion mode ----

fn wmtypes(seed: usize) -> ([TransformationType; 8], [i32; 8]) {
    let all = [
        TransformationType::Identity,
        TransformationType::Translation,
        TransformationType::RotZoom,
        TransformationType::Affine,
    ];
    let mut p_ty = [TransformationType::Identity; 8];
    let mut c_ty = [0i32; 8];
    for i in 0..8 {
        let t = all[(seed + i) % 4];
        p_ty[i] = t;
        c_ty[i] = t as i32;
    }
    (p_ty, c_ty)
}

#[test]
fn is_nontrans_global_motion_matches_c() {
    let mut trues = 0usize;
    let mut checked = 0usize;
    for seed in 0..4 {
        let (p_ty, c_ty) = wmtypes(seed);
        for bsize in BlockSize::ALL {
            for mode in [0u8, 13, 15, 16, 17, 23, 24] {
                for rf in [[1i8, -1], [1, 5], [4, 7], [5, 6], [0, -1]] {
                    // C indexes `pcs->global_motion[block_mi->ref_frame[ref]]`
                    // for `ref` in `0..=is_inter_compound_mode(mode)` WITHOUT
                    // checking the value, so a compound mode carrying
                    // `ref_frame[1] == NONE (-1)` reads one element BEFORE the
                    // array. That input is unreachable in the encoder (a
                    // compound mode always has two refs) and undefined in C, so
                    // it is not a comparable cell.
                    if p::modes::is_inter_compound_mode(mode) && rf[1] < 0 {
                        continue;
                    }
                    let got = p::interp::is_nontrans_global_motion(mode, bsize, rf, &p_ty);
                    let want = cref::is_nontrans_global_motion(
                        mode as i32,
                        bsize as i32,
                        rf[0] as i32,
                        rf[1] as i32,
                        &c_ty,
                    );
                    assert_eq!(
                        got, want,
                        "nontrans GM (seed={seed} {bsize:?} mode={mode} rf={rf:?})"
                    );
                    trues += usize::from(want);
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 1000, "only {checked} cells");
    assert!(
        trues > 0,
        "the gate never returned true — the probe cannot see a failure"
    );
}

#[test]
fn is_interintra_allowed_matches_c() {
    let mut trues = 0usize;
    for bsize in BlockSize::ALL {
        for mode in 0u8..25 {
            for rf in [[1i8, -1], [1, 0], [1, 5], [0, -1], [7, -1], [0, 0]] {
                let got = p::modes::is_interintra_allowed(bsize, mode, rf);
                let want = cref::is_interintra_allowed(
                    bsize as i32,
                    mode as i32,
                    rf[0] as i32,
                    rf[1] as i32,
                );
                assert_eq!(
                    got, want,
                    "interintra allowed ({bsize:?} mode={mode} rf={rf:?})"
                );
                trues += usize::from(want);
            }
        }
    }
    assert!(trues > 0, "the gate never returned true");
}

#[test]
fn motion_mode_allowed_matches_c() {
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..4 {
        let (p_ty, c_ty) = wmtypes(seed);
        for &switchable in &[true, false] {
            for &force_int in &[true, false] {
                for &warp in &[true, false] {
                    for npr in [0u16, 1, 3] {
                        for ovl in [0u32, 2] {
                            for bsize in [
                                BlockSize::Block4x4,
                                BlockSize::Block8x8,
                                BlockSize::Block16x16,
                                BlockSize::Block4x16,
                                BlockSize::Block64x64,
                            ] {
                                for mode in [13u8, 15, 16, 17, 23, 24] {
                                    for rf in [[1i8, -1], [1, 0], [1, 5], [4, -1]] {
                                        let got = p::modes::motion_mode_allowed(
                                            switchable, force_int, warp, &p_ty, npr, ovl, bsize,
                                            rf[0], rf[1], mode,
                                        );
                                        let want = cref::motion_mode_allowed(
                                            switchable,
                                            force_int,
                                            warp,
                                            &c_ty,
                                            npr as i32,
                                            ovl as i32,
                                            bsize as i32,
                                            rf[0] as i32,
                                            rf[1] as i32,
                                            mode as i32,
                                        );
                                        assert_eq!(
                                            got as i32, want,
                                            "motion_mode_allowed (seed={seed} sw={switchable} fi={force_int} warp={warp} npr={npr} ovl={ovl} {bsize:?} mode={mode} rf={rf:?})"
                                        );
                                        seen.insert(want);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(
        seen,
        [
            MotionMode::SimpleTranslation as i32,
            MotionMode::ObmcCausal as i32,
            MotionMode::WarpedCausal as i32
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>(),
        "the sweep must reach all three motion modes, else it cannot see a mis-gate"
    );
}

// ---- 7. the header bit-buffer signed recentred subexponential code ----

#[test]
fn wb_signed_refsubexpfin_matches_c() {
    use svtav1_encoder::entropy::obu::BitWriter;
    let mut checked = 0usize;
    let mut max_bits = 0usize;
    // The two (n, k) pairs write_global_motion_params actually uses, plus the
    // translation-only variant.
    let ns: [i32; 3] = [
        p::gm::GM_ALPHA_MAX as i32 + 1,
        (1 << p::gm::GM_ABS_TRANS_BITS) + 1,
        (1 << (p::gm::GM_ABS_TRANS_ONLY_BITS - 1)) + 1,
    ];
    for n in ns {
        let k = p::gm::SUBEXPFIN_K as i32;
        // A spread of (ref, v) across the whole signed range.
        let span = n - 1;
        let pts: Vec<i32> = [
            -span,
            -span / 2,
            -span / 7,
            -3,
            -1,
            0,
            1,
            3,
            span / 7,
            span / 2,
            span,
        ]
        .into_iter()
        .collect();
        for &r in &pts {
            for &v in &pts {
                let mut wb = BitWriter::new();
                p::gm::wb_write_signed_primitive_refsubexpfin(
                    &mut wb, n as u16, k as u16, r as i16, v as i16,
                );
                let (want_bits, want_bytes) = cref::wb_signed_refsubexpfin(n, k, r, v);
                assert_eq!(
                    wb.bit_len(),
                    want_bits,
                    "bit count differs (n={n} k={k} ref={r} v={v})"
                );
                assert_eq!(
                    wb.data(),
                    &want_bytes[..],
                    "bits differ (n={n} k={k} ref={r} v={v})"
                );
                max_bits = max_bits.max(want_bits);
                checked += 1;
            }
        }
    }
    assert!(checked >= 300, "only {checked} cells");
    assert!(
        max_bits > 8,
        "every case fitted in one byte — the sweep is too narrow"
    );
}

// ---- 8. the default CDF tables ----

fn assert_table(name: &str, got: &[u16], t: cref::EcFcTable) {
    let want = cref::ec_fc_table(t);
    assert_eq!(got.len(), want.len(), "{name}: length differs");
    assert_eq!(got, &want[..], "{name}: contents differ from C's defaults");
}

#[test]
fn default_inter_cdf_tables_match_c() {
    use svtav1_encoder::port_entropy_inter::cdfs as t;
    macro_rules! flat {
        ($tbl:expr) => {
            $tbl.iter().flatten().copied().collect::<Vec<u16>>()
        };
        (3 $tbl:expr) => {
            $tbl.iter()
                .flatten()
                .flatten()
                .copied()
                .collect::<Vec<u16>>()
        };
    }
    assert_table(
        "comp_ref_type",
        &flat!(t::COMP_REF_TYPE_CDF),
        cref::EcFcTable::CompRefType,
    );
    assert_table(
        "uni_comp_ref",
        &flat!(3 t::UNI_COMP_REF_CDF),
        cref::EcFcTable::UniCompRef,
    );
    assert_table(
        "comp_bwdref",
        &flat!(3 t::COMP_BWDREF_CDF),
        cref::EcFcTable::CompBwdRef,
    );
    assert_table(
        "skip_mode",
        &flat!(t::SKIP_MODE_CDF),
        cref::EcFcTable::SkipMode,
    );
    assert_table("newmv", &flat!(t::NEWMV_CDF), cref::EcFcTable::NewMv);
    assert_table("zeromv", &flat!(t::ZEROMV_CDF), cref::EcFcTable::ZeroMv);
    assert_table("refmv", &flat!(t::REFMV_CDF), cref::EcFcTable::RefMv);
    assert_table("drl", &flat!(t::DRL_CDF), cref::EcFcTable::Drl);
    assert_table(
        "inter_compound_mode",
        &flat!(t::INTER_COMPOUND_MODE_CDF),
        cref::EcFcTable::InterCompoundMode,
    );
    assert_table(
        "switchable_interp",
        &flat!(t::SWITCHABLE_INTERP_CDF),
        cref::EcFcTable::SwitchableInterp,
    );
    assert_table(
        "motion_mode",
        &flat!(t::MOTION_MODE_CDF),
        cref::EcFcTable::MotionMode,
    );
    assert_table("obmc", &flat!(t::OBMC_CDF), cref::EcFcTable::Obmc);
    assert_table(
        "compound_index",
        &flat!(t::COMPOUND_INDEX_CDF),
        cref::EcFcTable::CompoundIndex,
    );
    assert_table(
        "comp_group_idx",
        &flat!(t::COMP_GROUP_IDX_CDF),
        cref::EcFcTable::CompGroupIdx,
    );
    assert_table(
        "interintra",
        &flat!(t::INTERINTRA_CDF),
        cref::EcFcTable::InterIntra,
    );
    assert_table(
        "interintra_mode",
        &flat!(t::INTERINTRA_MODE_CDF),
        cref::EcFcTable::InterIntraMode,
    );
    assert_table(
        "wedge_interintra",
        &flat!(t::WEDGE_INTERINTRA_CDF),
        cref::EcFcTable::WedgeInterIntra,
    );
    assert_table(
        "wedge_idx",
        &flat!(t::WEDGE_IDX_CDF),
        cref::EcFcTable::WedgeIdx,
    );
    assert_table(
        "compound_type",
        &flat!(t::COMPOUND_TYPE_CDF),
        cref::EcFcTable::CompoundType,
    );
}

/// The three tables `write_ref_frames` reads off `FrameContext` rather than
/// [`svtav1_encoder::port_entropy_inter::InterCdfs`] must ALSO be C's
/// defaults, or the port's ref-frame tree codes against the wrong
/// probabilities even with this lane's tables correct.
#[test]
fn frame_context_ref_tables_match_c() {
    use svtav1_encoder::entropy::context::FrameContext;
    let fc = FrameContext::new_default();
    assert_table(
        "FrameContext::single_ref_cdf",
        &fc.single_ref_cdf
            .iter()
            .flatten()
            .flatten()
            .copied()
            .collect::<Vec<u16>>(),
        cref::EcFcTable::SingleRef,
    );
    assert_table(
        "FrameContext::comp_ref_cdf",
        &fc.comp_ref_cdf
            .iter()
            .flatten()
            .flatten()
            .copied()
            .collect::<Vec<u16>>(),
        cref::EcFcTable::CompRef,
    );
    assert_table(
        "FrameContext::comp_inter_cdf",
        &fc.comp_inter_cdf
            .iter()
            .flatten()
            .copied()
            .collect::<Vec<u16>>(),
        cref::EcFcTable::CompInter,
    );
}

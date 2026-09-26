use super::*;

/// The reference cell's two frames, exactly as `tools/identity_run`'s
/// `gradient` content and its `SVTAV1_FRAME_SHIFT` translate build them.
fn reference_cell(w: usize, h: usize, shift: usize) -> (Vec<u8>, Vec<u8>) {
    let f0: Vec<u8> = (0..h)
        .flat_map(|r| (0..w).map(move |c| (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8)))
        .collect();
    let mut f1 = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            f1[r * w + c] = f0[r * w + c.saturating_sub(shift)];
        }
    }
    (f0, f1)
}

/// **The FRAME-LEVEL driver recovers C's MV** — the same result
/// `pipeline::inter_decision_probe::the_ports_own_svt_motion_search_finds_
/// cs_mv_on_the_reference_cell` gets from a hand-built call, now through
/// the arm the encoder will use.
///
/// C's `SVT_CINTER_OUT` on `gradient 64x64 q40 p6 frames=2` prints
/// `mv0=0,-24` eighth-pel = the full-pel `(-3, 0)` this asserts.
///
/// Evidence tier 4 (`docs/WORKING-ON-THIS.md` §4): most of
/// `motion_estimation.c` is `static`, so this is a reachability and wiring
/// result, not a bit-exactness one. The kernels under it are tier 1
/// (`tests/c_parity_inter_me.rs`).
#[test]
fn the_frame_level_me_arm_finds_cs_mv_on_the_reference_cell() {
    const W: usize = 64;
    const H: usize = 64;
    const SHIFT: usize = 3;
    let (f0, f1) = reference_cell(W, H, SHIFT);

    let pa_ref = PaPicture::from_source(&f0, W, W, H, 0);
    let pa_cur = PaPicture::from_source(&f1, W, W, H, 1);

    // The left margin is what makes the -3 match EXACT: `identity_run`
    // builds frame 1 by replicating column 0, and C's own PA reference
    // replicates the same way (`svt_aom_generate_padding`).
    assert_eq!(
        pa_ref.full.buf[pa_ref.full.org - 3],
        f0[0],
        "the PA reference's left margin must replicate column 0"
    );

    let me = run_frame_me(
        &pa_cur,
        &pa_ref,
        FrameMeParams {
            enc_mode: 6,
            qp: 40,
            width: W,
            height: H,
            picture_number: 1,
            frame_is_boosted: false,
            hierarchical_levels: 0,
            sc_class5: 0,
            temporal_layer_index: 0,
            is_ref: true,
            // C `scs->mrp_ctrls` at this test's preset (level 6 for
            // enc_mode <= M8).
            only_l_bwd: true,
            safe_limit_nref: 2,
            safe_limit_zz_th: 60_000,
            similar_brightness_refs: false,
            frame_is_leaf: false,
            reference: crate::reference::SvtReference::Hybrid3115,
        },
    );
    assert_eq!((me.b64_cols, me.b64_rows), (1, 1));

    // C `BLOCK_64X64` / `BLOCK_32X32` / `BLOCK_16X16` / `BLOCK_8X8`.
    for (bsize, bw) in [(12u8, 64usize), (9, 32), (6, 16), (3, 8)] {
        for oy in (0..H).step_by(bw) {
            for ox in (0..W).step_by(bw) {
                let mv = me
                    .mv_for(ox, oy, bsize, 0, 0, 4)
                    .expect("every in-frame block has an ME slot");
                assert_eq!(
                    (mv.x, mv.y),
                    (-(SHIFT as i16), 0),
                    "block {bw}x{bw} at ({ox},{oy}) must recover the cell's full-pel MV"
                );
            }
        }
    }

    // POSITIVE CONTROL that the search area was installed: with the
    // signals bridge writing nothing a default `MeContext` has a ZERO
    // search area, in which no MV but (0,0) is reachable — so the
    // assertion above could only pass by the content being static.
    let (still0, still1) = (f0.clone(), f0.clone());
    let me_still = run_frame_me(
        &PaPicture::from_source(&still1, W, W, H, 1),
        &PaPicture::from_source(&still0, W, W, H, 0),
        FrameMeParams {
            enc_mode: 6,
            qp: 40,
            width: W,
            height: H,
            picture_number: 1,
            frame_is_boosted: false,
            hierarchical_levels: 0,
            sc_class5: 0,
            temporal_layer_index: 0,
            is_ref: true,
            // C `scs->mrp_ctrls` at this test's preset (level 6 for
            // enc_mode <= M8).
            only_l_bwd: true,
            safe_limit_nref: 2,
            safe_limit_zz_th: 60_000,
            similar_brightness_refs: false,
            frame_is_leaf: false,
            reference: crate::reference::SvtReference::Hybrid3115,
        },
    );
    assert_eq!(
        me_still.mv_for(0, 0, 12, 0, 0, 4).map(|m| (m.x, m.y)),
        Some((0, 0)),
        "an unmoved picture must give the zero MV — otherwise the -3 above \
             is noise, not a search result"
    );
}

/// **The PARTIAL-SUPERBLOCK cell, PINNED to C's own normalised distortions.**
///
/// C normalises every `me_*_distortion` by
/// `pix_num = b64_geom->width * b64_geom->height`
/// (`compute_distortion`, motion_estimation.c:2779), and `b64_geom`'s
/// dims are the CROPPED per-superblock extent,
/// `MIN(picture_dim - org, 64)` (pcs.c:1507-1508). This port carried the
/// whole PICTURE's dims in that field for every b64, so on a partial
/// superblock it divided by 28224 where C divides by 1600.
///
/// MEASURED 2026-09-02 on `gradient 168x168 q32 p8 frames=2` frame 1,
/// from C's own `SVT_PD0CFG_OUT` `med=` field (which prints
/// `ppcs->me_{64,32,16,8}x*_distortion[sb_index]` indexed explicitly, so
/// §5's "an interposer reads the context at its own call site" trap does
/// not apply). The six FULL superblocks report `med=0/0/0/0` on both
/// sides; the three partial ones are asserted here.
///
/// OBSERVED BEFORE, against the same C values:
/// ```text
///   b64 2 (128,0)   C 36736/35776/32640/23584   port 3332/3244/2960/2139
///   b64 5 (128,64)  C 37990/37990/35699/25299   port 3445/3445/3238/2294
///   b64 8 (128,128) C 52326/51640/47933/35553   port 2966/2927/2717/2015
/// ```
/// The ratios — 11.02, 11.03 and 17.64 — are exactly
/// `(4096/pix_num_C) / (4096/28224)` for `pix_num_C` of 2560, 2560 and
/// 1600.
///
/// **`me_8x8_cost_variance` is asserted alongside BECAUSE IT NEVER MOVED**
/// (it matched C exactly on all nine superblocks before and after): it is
/// computed from the RAW `me_distortion[]` array before any normalisation,
/// so it is the one statistic out of that function this defect could not
/// touch — and it is why the defect survived, since it was the statistic
/// that had been checked. A cell that asserted only the variance would
/// have passed throughout.
///
/// This fix is BYTE-INERT on everything measured (inter byte gate 55
/// required / 0 failed, completion grid's 5 identical cells unchanged,
/// `identity_full_8bit` 1100/1100, `video_key_matrix` 58/60, and the four
/// 40-remainder cells emit the same frame-1 bytes before and after), which
/// is exactly why it is gated HERE and has no `regression_spotcheck.sh`
/// cell: per §3 a cell must have failed before and passed after, and no
/// byte comparison did.
///
/// Evidence tier 2.
#[test]
fn a_partial_superblocks_distortions_are_normalised_by_its_own_cropped_extent() {
    const W: usize = 168;
    const H: usize = 168;
    let (f0, f1) = reference_cell(W, H, 3);
    let me = run_frame_me(
        &PaPicture::from_source(&f1, W, W, H, 1),
        &PaPicture::from_source(&f0, W, W, H, 0),
        FrameMeParams {
            enc_mode: 8,
            qp: 32,
            width: W,
            height: H,
            picture_number: 1,
            frame_is_boosted: false,
            hierarchical_levels: 0,
            sc_class5: 0,
            temporal_layer_index: 0,
            is_ref: true,
            // C `scs->mrp_ctrls` at this test's preset (level 6 for
            // enc_mode <= M8).
            only_l_bwd: true,
            safe_limit_nref: 2,
            safe_limit_zz_th: 60_000,
            similar_brightness_refs: false,
            frame_is_leaf: false,
            reference: crate::reference::SvtReference::Hybrid3115,
        },
    );
    assert_eq!((me.b64_cols, me.b64_rows), (3, 3));
    // (b64 index, C's med=64/32/16/8, C's mev)
    let expected: [(usize, [u32; 4], u32); 3] = [
        (2, [36736, 35776, 32640, 23584], 110342),
        (5, [37990, 37990, 35699, 25299], 125504),
        (8, [52326, 51640, 47933, 35553], 97383),
    ];
    for (b64, med, mev) in expected {
        let o = &me.per_b64[b64];
        assert_eq!(
            [
                o.me_64x64_distortion,
                o.me_32x32_distortion,
                o.me_16x16_distortion,
                o.me_8x8_distortion
            ],
            med,
            "b64 {b64}: C's normalised me_*_distortion"
        );
        assert_eq!(
            o.me_8x8_cost_variance, mev,
            "b64 {b64}: the variance is normalisation-free and matched C                  before this fix too — it is the control, not the assertion"
        );
    }
    // The six COMPLETE superblocks: C reports med=0/0/0/0 on each, and a
    // fix that normalised everything by 64x64 unconditionally would keep
    // them at 0 too. They are here so the cell says what it does NOT
    // separate.
    for b64 in [0usize, 1, 3, 4, 6, 7] {
        let o = &me.per_b64[b64];
        assert_eq!(o.me_64x64_distortion, 0, "b64 {b64}");
        assert_eq!(o.me_8x8_distortion, 0, "b64 {b64}");
    }
}

/// **The two-SB-column cell, PINNED to values read out of the real C
/// encoder** through `SVT_HME_OUT` (`tools/capture_c_trace/wrap_recon.c`'s
/// `__wrap_svt_aom_motion_estimation_b64`), which is the only exported
/// vantage point on `motion_estimation.c`'s otherwise-`static` pyramid.
///
/// Measured 2026-09-02, `gradient 128x128 q40 p8 frames=2` frame 1, all
/// four b64s, on the container oracle (`tools/ctrace-linux/run.sh`):
///
/// ```text
/// MESIG hme=1/1/1/0 mesa=10x4/15x8 l0sa=10x10/122x122 ubuc=1 nlist=2 nref=1/1
/// MERES b64=0 d64=0 bestsad64=18816 bestmv64=(40,0)  mecand=1
/// MERES b64=1 d64=0 bestsad64=13312 bestmv64=(-24,0) mecand=1
/// MEL1  b64=* l1sad64=0 l1mv64=(-3,0) l1hme=(0,0):0 mv0=(0,0) mvl1=(-3,0)
/// ```
///
/// Every assertion below is one of those numbers, and each one has TEETH
/// against a specific defect this chunk fixed
/// (`docs/INTER-ENCODE-PLAN.md` §1z¹³):
///
/// * `me_64x64_distortion == 0` and `cand_mv_for == (1, (-3,0))` fail if
///   list 1 is not searched (`num_of_list_to_search = 1`) — the port then
///   reports 3328/4704 and candidate direction 0.
/// * `mv_for(list 0) == (0,0)` on the x=64 column fails if
///   `enable_hme_level2_flag` is wrongly 1: level 2 refines list 0 all the
///   way to (-3,0) and the list-0 slot stops being C's.
///
/// It does NOT witness the qp-based search-area scaling: turning that flag
/// back off leaves every assertion here green (measured). The scaling has
/// its own cell, [`the_search_areas_join_cs_measured_mesig_line`].
///
/// Evidence tier 2 (`docs/WORKING-ON-THIS.md` §4) — the constants come
/// from the real `libSvtAv1Enc.a` running the real encoder, but through a
/// link-time interposer on an outer entry point, not a per-function
/// differential.
#[test]
fn the_me_arm_joins_cs_measured_two_sb_column_state() {
    const W: usize = 128;
    const H: usize = 128;
    let (f0, f1) = reference_cell(W, H, 3);
    let me = run_frame_me(
        &PaPicture::from_source(&f1, W, W, H, 1),
        &PaPicture::from_source(&f0, W, W, H, 0),
        FrameMeParams {
            enc_mode: 8,
            qp: 40,
            width: W,
            height: H,
            picture_number: 1,
            frame_is_boosted: false,
            hierarchical_levels: 0,
            sc_class5: 0,
            temporal_layer_index: 0,
            is_ref: true,
            // C `scs->mrp_ctrls` at this test's preset (level 6 for
            // enc_mode <= M8).
            only_l_bwd: true,
            safe_limit_nref: 2,
            safe_limit_zz_th: 60_000,
            similar_brightness_refs: false,
            frame_is_leaf: false,
            reference: crate::reference::SvtReference::Hybrid3115,
        },
    );
    assert_eq!((me.b64_cols, me.b64_rows), (2, 2));
    for (b64, (org_x, org_y)) in [(0, 0), (64, 0), (0, 64), (64, 64)].into_iter().enumerate() {
        let out = &me.per_b64[b64];
        assert_eq!(
            out.me_64x64_distortion, 0,
            "b64 {b64}: C reports med=0/0/0/0 on every superblock"
        );
        assert_eq!(out.me_32x32_distortion, 0, "b64 {b64}");
        assert_eq!(out.me_8x8_cost_variance, 0, "b64 {b64}");
        assert_eq!(
            out.total_me_candidate_index[0], 1,
            "b64 {b64}: C emits ONE me candidate (list 0 is pruned by \
                 prune_me_candidates_th against list 1's zero distortion)"
        );
        assert_eq!(
            me.cand_mv_for(org_x, org_y, 12, 0)
                .map(|(d, m)| (d, m.x, m.y)),
            Some((1, -3, 0)),
            "b64 {b64}: C's surviving candidate is LIST 1's, at the cell's \
                 true full-pel MV"
        );
        // C's list-0 slot: written on neither column, because list 0 is
        // pruned before `construct_me_candidate_array_mrp_off` writes it.
        assert_eq!(
            me.mv_for(org_x, org_y, 12, 0, 0, me.max_l0)
                .map(|m| (m.x, m.y)),
            Some((0, 0)),
            "b64 {b64}: C leaves the list-0 me_mv_array slot untouched"
        );
    }
}

/// **The resolved ME/HME search areas, PINNED to C's `MESIG` line.**
/// Measured 2026-09-02 through `SVT_HME_OUT` on a 128x128 preset-8
/// low-delay-P encode:
///
/// ```text
/// q40: mesa=10x4/15x8   l0sa=10x10/122x122   hme=1/1/1/0   ubuc=1
/// q55: mesa=15x5/22x11  l0sa=15x15/176x176   hme=1/1/1/0   ubuc=1
/// ```
///
/// TEETH: with `me_qp_based_th_scaling` / `hme_qp_based_th_scaling` forced
/// back to `false` — the value the driver passed before §1z¹³ — this reads
/// `mesa=16x6/24x12` and `l0sa=16x16/192x192` at BOTH qps, and every
/// assertion below fails. Evidence tier 2.
#[test]
fn the_search_areas_join_cs_measured_mesig_line() {
    let res = ResolutionRange::from_luma_area(128 * 128);
    let params = |qp: u8| FrameMeParams {
        enc_mode: 8,
        qp,
        width: 128,
        height: 128,
        picture_number: 1,
        frame_is_boosted: false,
        hierarchical_levels: 0,
        sc_class5: 0,
        temporal_layer_index: 0,
        is_ref: true,
        // C `scs->mrp_ctrls` at this test's preset (level 6 for
        // enc_mode <= M8).
        only_l_bwd: true,
        safe_limit_nref: 2,
        safe_limit_zz_th: 60_000,
        similar_brightness_refs: false,
        frame_is_leaf: false,
        reference: crate::reference::SvtReference::Hybrid3115,
    };
    for (qp, sa_min, sa_max, l0sa) in [
        (
            40u8,
            (10u16, 4u16),
            (15u16, 8u16),
            (10u16, 10u16, 122u16, 122u16),
        ),
        (55, (15, 5), (22, 11), (15, 15, 176, 176)),
    ] {
        let s = sig_deriv_me(
            me_deriv_inputs(params(qp), res),
            crate::reference::SvtReference::Hybrid3115,
        );
        assert_eq!(
            (s.me_sa.sa_min.width, s.me_sa.sa_min.height),
            sa_min,
            "q{qp} me_sa.sa_min"
        );
        assert_eq!(
            (s.me_sa.sa_max.width, s.me_sa.sa_max.height),
            sa_max,
            "q{qp} me_sa.sa_max"
        );
        assert_eq!(
            (
                s.hme.hme_l0_sa.sa_min.width,
                s.hme.hme_l0_sa.sa_min.height,
                s.hme.hme_l0_sa.sa_max.width,
                s.hme.hme_l0_sa.sa_max.height
            ),
            l0sa,
            "q{qp} hme_l0_sa"
        );
        assert_eq!(
            (
                s.enable_hme_flag,
                s.enable_hme_level0_flag,
                s.enable_hme_level1_flag,
                s.enable_hme_level2_flag
            ),
            (1, 1, 1, 0),
            "q{qp} HME flags — C reports hme=1/1/1/0 at preset 8"
        );
        assert_eq!(s.use_best_unipred_cand_only, 1, "q{qp} ubuc");
    }
}

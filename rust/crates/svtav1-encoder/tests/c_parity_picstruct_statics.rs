//! Differential tests for the three `static` `Codec/pd_process.c` functions
//! that `svtav1-cref`'s build script promotes to linkable symbols —
//! evidence **tier 1** (`docs/WORKING-ON-THIS.md` §4).
//!
//! `set_ref_list_counts`, `set_all_ref_frame_type` and
//! `scene_transition_detector` have no global symbol in
//! `Bin/Release/libSvtAv1Enc.a` (`nm -g` finds nothing), so the obvious
//! conclusion is "tier 4, nothing to be done". They DO survive in
//! `cbuild-static/.../pd_process.c.o` as local (`t`) symbols, and
//! `llvm-objcopy --globalize-symbol` on a private copy of that object promotes
//! them without touching the C tree or the archive. Linking the promoted
//! object alongside the archive produces no duplicate symbols, because the
//! object supplies everything the archive member would have and the member is
//! therefore never pulled in.
//!
//! That makes these two the highest-value tier-4 entries in the
//! picture-decision group reachable at tier 1: `set_ref_list_counts` decides
//! how many references each list signals, and `set_all_ref_frame_type` decides
//! the exact ordered candidate set MD walks.
//!
//! **This is an ADDITIVE gate, not a conditional one.** The same two
//! functions are covered unconditionally at tier 4 in
//! `port_picstruct_traced.rs`, which always runs; the differentials here are a
//! strictly stronger oracle on hosts that can reach it. So a host without the
//! object loses evidence STRENGTH, never coverage — this is not a test that
//! passes without testing anything.
//!
//! Why it cannot simply be required everywhere, measured from the workflow
//! rather than assumed: `.github/workflows/rust-gates.yml` caches `Bin/Release`
//! and DELIBERATELY not `cbuild-static`, so on a cache hit build.rs does
//! nothing and the object is absent — the same job flips between present and
//! absent with the cache. Requiring it by default would be flaky, not strict.
//! `SVT_CREF_REQUIRE_PICSTRUCT_STATICS=1` makes it strict where the caller
//! knows the object is there, matching the workflow's existing
//! `ZENAV1_SKIP_*` convention (workflow -> env -> test, never inside a test).
//!
//! **The hazard this technique has, measured here and not hypothetical.**
//! Globalizing a symbol makes it LINKABLE; it does not make the source
//! signature correct. LLVM may change an `internal` function's calling
//! convention, and for `scene_transition_detector` it did — the compiled
//! symbol takes the CURRENT `PictureParentControlSet*` where the source takes
//! the three-picture window array (argument promotion of `window[1]`), so
//! calling it as declared reads `enhanced_pic` out of the array and segfaults
//! on `NULL+0x68`. That function is therefore NOT gated here and stays at
//! tier 4. The two below were checked by disassembling their prologues before
//! being trusted: `set_ref_list_counts` opens `ldrb w8, [x0, #0xe8]`
//! (`PPCS.slice_type`, offset 232) so `x0` is the PPCS and `x1` the context,
//! and `set_all_ref_frame_type` uses `x0..x3` for its four source parameters.
//! **Disassemble the prologue before trusting a globalized static.**

use svtav1_cref::picstruct as cref;

/// The availability gate. Always compiled, so an unavailable oracle is a
/// visible fact rather than an absent test.
// `PICSTRUCT_STATICS_AVAILABLE` is a const, so the assertion below is a
// constant one — it IS, per build. That is the point: the build script decides
// whether the stronger oracle exists and the caller decides whether to demand
// it.
#[allow(clippy::assertions_on_constants)]
#[test]
fn picstruct_statics_oracle_is_available() {
    let required = std::env::var("SVT_CREF_REQUIRE_PICSTRUCT_STATICS")
        .map(|v| v == "1")
        .unwrap_or(false);
    if required {
        assert!(
            cref::PICSTRUCT_STATICS_AVAILABLE,
            "SVT_CREF_REQUIRE_PICSTRUCT_STATICS=1 but the build script could not promote the \
             `static` pd_process.c symbols. Its cargo:warning names the reason: either \
             <repo>/cbuild-static/Source/Lib/Codec/CMakeFiles/CODEC.dir/pd_process.c.o is \
             missing (build the C library into cbuild-static, or stop pointing \
             SVT_CREF_LIB_DIR at a prebuilt archive) or no llvm-objcopy was found (set \
             $LLVM_OBJCOPY)."
        );
    }
}

mod differential {
    use super::cref;
    use svtav1_encoder::port_picstruct as pp;

    /// `set_ref_list_counts` over a POC grid built to hit every de-duplication
    /// path in BOTH list loops, crossed with the MRP caps and the boosted /
    /// non-boosted split.
    ///
    /// The POC sets are chosen so that list 0 breaks out at LAST2, at LAST3, at
    /// GOLD and not at all, and so that list 1's BWD row (which starts at
    /// LAST2, not LAST) is exercised both matching and not matching.
    #[test]
    fn c_parity_set_ref_list_counts_grid() {
        if !cref::PICSTRUCT_STATICS_AVAILABLE {
            // Not a silent skip: `picstruct_statics_oracle_is_available`
            // reports the state in this same binary, the build script printed
            // a cargo:warning naming the missing piece, and the tier-4 traced
            // coverage of this function runs unconditionally elsewhere.
            eprintln!(
                "c_parity_set_ref_list_counts_grid: promoted pd_process.c statics unavailable \
                 on this host; tier-4 coverage in port_picstruct_traced.rs still applies"
            );
            return;
        }
        let poc_sets: [[u64; 7]; 12] = [
            [9, 9, 9, 9, 9, 9, 9],
            [4, 3, 2, 1, 4, 4, 4],
            [4, 3, 2, 1, 8, 9, 10],
            [4, 4, 2, 1, 8, 9, 10],
            [4, 3, 3, 1, 8, 9, 10],
            [4, 3, 2, 2, 8, 9, 10],
            [1, 0, 0, 0, 0, 0, 0],
            [1, 0, 1, 0, 1, 0, 1],
            [0, 1, 2, 3, 4, 5, 6],
            [6, 5, 4, 3, 2, 1, 0],
            [10, 10, 10, 10, 20, 20, 20],
            [10, 9, 8, 7, 10, 9, 8],
        ];
        // (frame_type, update_type) pairs that straddle frame_is_boosted.
        let modes: [(u8, u8, bool, pp::FrameUpdateType); 4] = [
            (1, 1, false, pp::FrameUpdateType::Lf),
            (1, 2, false, pp::FrameUpdateType::Gf),
            (1, 3, false, pp::FrameUpdateType::Arf),
            (0, 0, true, pp::FrameUpdateType::Kf),
        ];
        let mut saw_l1_zero = false;
        let mut saw_l1_nonzero = false;
        let mut saw_l0_capped = false;
        for poc in poc_sets {
            for (frame_type, update_type, intra_only, rust_ut) in modes {
                for slice_type in [0u8, 1] {
                    for overlay in [false, true] {
                        for (pic_ps, seq_ps) in [(1u8, 1u8), (1, 2), (2, 2), (2, 1)] {
                            for (b0, b1, n0, n1) in
                                [(4u8, 3u8, 4u8, 3u8), (2, 1, 1, 1), (4, 3, 2, 2)]
                            {
                                let seq = pp::SeqPicParams {
                                    pred_structure: match seq_ps {
                                        1 => pp::PredStructure::LowDelay,
                                        _ => pp::PredStructure::RandomAccess,
                                    },
                                    mrp_ctrls: pp::MrpCtrls {
                                        base_ref_list0_count: b0,
                                        base_ref_list1_count: b1,
                                        non_base_ref_list0_count: n0,
                                        non_base_ref_list1_count: n1,
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                };
                                let ctx = pp::PicDecisionCtx::new();
                                let mut pic = pp::PicParams {
                                    picture_number: 42,
                                    slice_type: if slice_type == 0 {
                                        pp::SliceType::B
                                    } else {
                                        pp::SliceType::I
                                    },
                                    is_intra_only: intra_only,
                                    update_type: rust_ut,
                                    is_overlay: overlay,
                                    pred_struct_type: match pic_ps {
                                        1 => pp::PredStructure::LowDelay,
                                        _ => pp::PredStructure::RandomAccess,
                                    },
                                    rps: pp::Av1RpsNode {
                                        ref_poc_array: poc,
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                };
                                pp::set_ref_list_counts(&mut pic, &seq, &ctx);
                                let want = cref::set_ref_list_counts(
                                    slice_type,
                                    frame_type,
                                    update_type,
                                    overlay,
                                    pic_ps,
                                    seq_ps,
                                    &poc,
                                    b0,
                                    b1,
                                    n0,
                                    n1,
                                    42,
                                    0,
                                )
                                .expect("availability was checked at the top of this test");
                                assert_eq!(
                                    (pic.ref_list0_count, pic.ref_list1_count),
                                    want,
                                    "set_ref_list_counts(poc={poc:?}, ft={frame_type}, \
                                     ut={update_type}, slice={slice_type}, overlay={overlay}, \
                                     pic_ps={pic_ps}, seq_ps={seq_ps}, \
                                     caps=({b0},{b1},{n0},{n1}))"
                                );
                                if pic.ref_list1_count == 0 {
                                    saw_l1_zero = true;
                                } else {
                                    saw_l1_nonzero = true;
                                }
                                if pic.ref_list0_count > 0 && pic.ref_list0_count < 4 {
                                    saw_l0_capped = true;
                                }
                            }
                        }
                    }
                }
            }
        }
        // Positive controls: the grid reaches an empty list 1, a non-empty one,
        // and a capped list 0. Without these an all-zeros port could pass.
        assert!(saw_l1_zero && saw_l1_nonzero && saw_l0_capped);
    }

    /// `set_all_ref_frame_type` over EVERY reachable `(slice_type, l0_try,
    /// l1_try)` — the exact ordered candidate set, element by element.
    #[test]
    fn c_parity_set_all_ref_frame_type_exhaustive() {
        if !cref::PICSTRUCT_STATICS_AVAILABLE {
            eprintln!(
                "c_parity_set_all_ref_frame_type_exhaustive: promoted pd_process.c statics \
                 unavailable on this host; tier-4 coverage in port_picstruct_traced.rs still \
                 applies"
            );
            return;
        }
        let mut max_seen = 0usize;
        for slice_type in [0u8, 1] {
            for l0 in 0u8..=4 {
                for l1 in 0u8..=3 {
                    let pic = pp::PicParams {
                        slice_type: if slice_type == 0 {
                            pp::SliceType::B
                        } else {
                            pp::SliceType::I
                        },
                        ref_list0_count_try: l0,
                        ref_list1_count_try: l1,
                        ..Default::default()
                    };
                    let (arr, tot) = pp::set_all_ref_frame_type(&pic);
                    let want = cref::set_all_ref_frame_type(slice_type, l0, l1)
                        .expect("availability was checked at the top of this test");
                    assert_eq!(
                        usize::from(tot),
                        want.len(),
                        "set_all_ref_frame_type(slice={slice_type}, l0={l0}, l1={l1}) count"
                    );
                    assert_eq!(
                        &arr[..usize::from(tot)],
                        &want[..],
                        "set_all_ref_frame_type(slice={slice_type}, l0={l0}, l1={l1}) SET"
                    );
                    max_seen = max_seen.max(want.len());
                }
            }
        }
        // Positive control: the sweep reaches the full 23-candidate set, so a
        // port that produced only the singles could not pass.
        assert_eq!(max_seen, 23);
    }
}

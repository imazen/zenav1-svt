use super::*;

/// The nine wedge-carrying sizes are exactly the ones
/// `is_interinter_compound_used(WEDGE, ·)` admits; every other size
/// admits the three unmasked types iff `min(w,h) >= 8`.
#[test]
fn wedge_availability_tracks_the_codebook_table() {
    for i in 0..22u8 {
        let Some(b) = BlockSize::from_u8(i) else {
            continue;
        };
        let wedge = is_interinter_compound_used(CompoundType::Wedge, b);
        assert_eq!(
            wedge,
            is_comp_ref_allowed(b) && get_wedge_params_bits(b.as_index()) > 0,
            "bsize {i}"
        );
        for t in [
            CompoundType::Average,
            CompoundType::DistWtd,
            CompoundType::DiffWtd,
        ] {
            assert_eq!(
                is_interinter_compound_used(t, b),
                is_comp_ref_allowed(b),
                "bsize {i} type {t:?}"
            );
        }
        // DIFFWTD is masked and needs only comp_ref_allowed, so
        // any-masked is exactly comp_ref_allowed.
        assert_eq!(is_any_masked_compound_used(b), is_comp_ref_allowed(b));
    }
}

#[test]
fn masked_types_are_wedge_and_diffwtd_only() {
    assert!(!is_masked_compound_type(CompoundType::Average));
    assert!(!is_masked_compound_type(CompoundType::DistWtd));
    assert!(is_masked_compound_type(CompoundType::Wedge));
    assert!(is_masked_compound_type(CompoundType::DiffWtd));
}

/// Step 7's gate is a hard early-out: neither the symbol NOR the
/// `rf[1] = INTRA_FRAME` assignment happens when it fails. Getting that
/// backwards would suppress step 8 on every block of a sequence that
/// disables interintra.
#[test]
fn interintra_gate_suppresses_the_ref_frame_mutation_too() {
    let mut ic = InterCdfs::new_default();
    let mut w = AomWriter::new(64);
    let mut rf = [1i8, -1];
    let used = write_interintra_info(
        &mut w,
        &mut ic,
        BlockSize::Block16x16,
        &mut rf,
        false,
        true,
        Some(InterIntraInfo {
            mode: InterIntraMode::IiVPred,
            use_wedge: false,
            wedge_index: 0,
        }),
    );
    assert!(!used);
    assert_eq!(rf, [1, -1], "no mutation behind a closed gate");
    assert_eq!(w.bytes_written(), 0, "no symbol behind a closed gate");
}

/// An interintra block sets `rf[1]` to INTRA_FRAME, which is what makes
/// `write_modes_b` skip step 8 (`rf[1] != INTRA_FRAME`).
#[test]
fn interintra_sets_ref_frame_one_to_intra() {
    let mut ic = InterCdfs::new_default();
    let mut w = AomWriter::new(64);
    let mut rf = [1i8, -1];
    let used = write_interintra_info(
        &mut w,
        &mut ic,
        BlockSize::Block16x16,
        &mut rf,
        true,
        true,
        Some(InterIntraInfo {
            mode: InterIntraMode::IiSmoothPred,
            use_wedge: true,
            wedge_index: 9,
        }),
    );
    assert!(used);
    assert_eq!(rf[1], INTRA_FRAME);
}

/// Tier 4, traced against entropy_coding.c:5245-5272: a 16x16 interintra
/// block with a wedge emits FOUR symbols (flag, mode, wedge flag, index)
/// and a non-wedge one emits TWO. The count is what a decoder desync
/// hinges on, so it is asserted through the op count rather than bytes.
#[test]
fn interintra_symbol_counts_match_the_c_branch_structure() {
    fn ops(use_wedge: bool, bsize: BlockSize) -> usize {
        let mut ic = InterCdfs::new_default();
        let mut w = AomWriter::new(64);
        let mut rf = [1i8, -1];
        let before = cdf_fingerprint(&ic);
        write_interintra_info(
            &mut w,
            &mut ic,
            bsize,
            &mut rf,
            true,
            true,
            Some(InterIntraInfo {
                mode: InterIntraMode::IiHPred,
                use_wedge,
                wedge_index: 3,
            }),
        );
        let after = cdf_fingerprint(&ic);
        before.iter().zip(after).filter(|(a, b)| **a != *b).count()
    }
    // 16x16 has a wedge codebook: flag + mode + wedge-flag [+ index].
    assert_eq!(ops(false, BlockSize::Block16x16), 3);
    assert_eq!(ops(true, BlockSize::Block16x16), 4);
    // 4x8 has none, and is not interintra-allowed either, but the
    // wedge sub-gate alone drops the last two symbols: flag + mode.
    assert_eq!(ops(false, BlockSize::Block64x64), 2);
}

/// Each adapted CDF row is one written symbol; the fingerprint is the
/// first element of every row this module can touch.
fn cdf_fingerprint(ic: &InterCdfs) -> alloc::vec::Vec<u16> {
    let mut v = alloc::vec::Vec::new();
    v.extend(ic.interintra_cdf.iter().map(|r| r[0]));
    v.extend(ic.interintra_mode_cdf.iter().map(|r| r[0]));
    v.extend(ic.wedge_interintra_cdf.iter().map(|r| r[0]));
    v.extend(ic.wedge_idx_cdf.iter().map(|r| r[0]));
    v.extend(ic.comp_group_idx_cdf.iter().map(|r| r[0]));
    v.extend(ic.compound_index_cdf.iter().map(|r| r[0]));
    v.extend(ic.compound_type_cdf.iter().map(|r| r[0]));
    v
}

/// Tier 4, traced against entropy_coding.c:5279-5342. Group A with
/// `enable_jnt_comp` off writes NOTHING beyond the group symbol; group B
/// on a wedge-capable size writes type + index (+ a raw sign bit, which
/// does not touch a CDF).
#[test]
fn compound_type_symbol_counts_match_the_c_branch_structure() {
    fn adapted(bsize: BlockSize, jnt: bool, group: CompGroup) -> usize {
        let mut ic = InterCdfs::new_default();
        let mut w = AomWriter::new(64);
        let before = cdf_fingerprint(&ic);
        write_compound_type_info(&mut w, &mut ic, bsize, true, jnt, 0, 0, group);
        let after = cdf_fingerprint(&ic);
        before.iter().zip(after).filter(|(a, b)| **a != *b).count()
    }
    // group symbol only.
    assert_eq!(
        adapted(
            BlockSize::Block16x16,
            false,
            CompGroup::A { compound_idx: true }
        ),
        1
    );
    // group symbol + compound_idx.
    assert_eq!(
        adapted(
            BlockSize::Block16x16,
            true,
            CompGroup::A { compound_idx: true }
        ),
        2
    );
    // group symbol + compound_type + wedge_idx.
    assert_eq!(
        adapted(
            BlockSize::Block16x16,
            true,
            CompGroup::B(InterInterComp {
                comp_type: CompoundType::Wedge,
                wedge_index: 5,
                wedge_sign: true,
                mask_type: 0,
            })
        ),
        3
    );
    // 64x64 has NO wedge codebook, so the compound_type symbol is
    // skipped and DIFFWTD is implicit: group symbol only, then a raw
    // literal that adapts nothing.
    assert_eq!(
        adapted(
            BlockSize::Block64x64,
            true,
            CompGroup::B(InterInterComp {
                comp_type: CompoundType::DiffWtd,
                wedge_index: 0,
                wedge_sign: false,
                mask_type: 1,
            })
        ),
        1
    );
}

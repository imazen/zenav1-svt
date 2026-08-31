//! Differential parity for the wedge mask tables — evidence tier 1
//! (`WORKING-ON-THIS.md` §4).
//!
//! Symbols driven: `svt_av1_init_wedge_masks` (which runs the `static`
//! `init_wedge_primary_masks`, `get_wedge_mask_inplace`, `init_wedge_masks`,
//! `shift_copy` and `aom_convolve_copy_c`), then
//! `svt_aom_get_contiguous_soft_mask`, `svt_aom_is_interintra_wedge_used`,
//! `svt_aom_get_wedge_bits_lookup` and `svt_aom_get_wedge_params_bits`.
//!
//! The five `static` builders are gated INDIRECTLY but COMPLETELY: every byte
//! of every mask C produces is compared, for all nine wedge-capable block
//! sizes, all 16 indices and both signs — 288 masks, 63,488 bytes. Any
//! difference in the shift ladder, the transposes, the `neg ^ wsignflip`
//! plane choice or the `MASK_PRIMARY_SIZE / 2 - hoff` offset shows up here.
//!
//! The `#else` arms of `init_wedge_primary_masks` (sqrt/tanh/rint on doubles)
//! and `init_wedge_signs` are NOT ported and NOT tested: both are behind
//! `USE_PRECOMPUTED_WEDGE_MASK` / `USE_PRECOMPUTED_WEDGE_SIGN`, which are 1
//! (inter_prediction.c:1514-1515), so neither compiles into the oracle either.

use svtav1_cref::inter_pred as cref;
use svtav1_dsp::port_wedge_masks::{
    BLOCK_SIZES_ALL, MAX_WEDGE_TYPES, WedgeMasks, get_wedge_bits_lookup, get_wedge_params_bits,
    is_interintra_wedge_used,
};

const BLOCK_W: [usize; BLOCK_SIZES_ALL] = [
    4, 4, 8, 8, 8, 16, 16, 16, 32, 32, 32, 64, 64, 64, 128, 128, 4, 16, 8, 32, 16, 64,
];
const BLOCK_H: [usize; BLOCK_SIZES_ALL] = [
    4, 8, 4, 8, 16, 8, 16, 32, 16, 32, 64, 32, 64, 128, 64, 128, 16, 4, 32, 8, 64, 16,
];

#[test]
fn wedge_bits_lookups_match_c() {
    for b in 0..BLOCK_SIZES_ALL as i32 {
        assert_eq!(
            is_interintra_wedge_used(b as usize),
            cref::is_interintra_wedge_used(b),
            "is_interintra_wedge_used bsize {b}"
        );
        assert_eq!(
            get_wedge_bits_lookup(b as usize),
            cref::get_wedge_bits_lookup(b),
            "get_wedge_bits_lookup bsize {b}"
        );
        assert_eq!(
            get_wedge_params_bits(b as usize),
            cref::get_wedge_params_bits(b),
            "get_wedge_params_bits bsize {b}"
        );
    }
}

/// Every byte of every wedge mask, against C's own initialised tables.
#[test]
fn every_wedge_mask_matches_c() {
    let m = WedgeMasks::new();
    let mut masks = 0usize;
    let mut bytes = 0usize;
    for bsize in 0..BLOCK_SIZES_ALL {
        if !is_interintra_wedge_used(bsize) {
            continue;
        }
        let n = BLOCK_W[bsize] * BLOCK_H[bsize];
        for idx in 0..MAX_WEDGE_TYPES {
            for sign in 0..2usize {
                let got = m.contiguous_soft_mask(idx, sign, bsize);
                let want = cref::get_contiguous_soft_mask(idx as i32, sign as i32, bsize as i32, n);
                assert_eq!(
                    got,
                    &want[..],
                    "wedge mask bsize {bsize} idx {idx} sign {sign}"
                );
                masks += 1;
                bytes += n;
            }
        }
    }
    assert_eq!(
        masks,
        9 * MAX_WEDGE_TYPES * 2,
        "expected 288 masks, compared {masks}"
    );
    assert!(bytes > 60_000, "only {bytes} mask bytes compared");
}

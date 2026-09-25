use super::*;

/// Duplicated from `crate::cdef`'s private `CDEF_DIRECTIONS_PADDED` (not
/// `pub`) — identical values, same C provenance
/// (`eb_cdef_directions_padded`, cdef.c:35).
pub(super) const CDEF_DIRECTIONS_PADDED_HBD: [[i32; 2]; 12] = {
    const S: i32 = CDEF_BSTRIDE as i32;
    [
        [S, 2 * S],
        [S, 2 * S - 1],
        [-S + 1, -2 * S + 2],
        [1, -S + 2],
        [1, 2],
        [1, S + 2],
        [S + 1, 2 * S + 2],
        [S, 2 * S + 1],
        [S, 2 * S],
        [S, 2 * S - 1],
        [-S + 1, -2 * S + 2],
        [1, -S + 2],
    ]
};

#[inline]
pub(super) fn cdef_direction_hbd(dir: i32, k: usize) -> i32 {
    CDEF_DIRECTIONS_PADDED_HBD[(dir + 2) as usize][k]
}

/// Duplicated from `crate::cdef`'s private `CDEF_PRI_TAPS`/`CDEF_SEC_TAPS`
/// (cdef.c:189-190).
pub(super) const CDEF_PRI_TAPS_HBD: [[i32; 2]; 2] = [[4, 2], [3, 3]];
pub(super) const CDEF_SEC_TAPS_HBD: [[i32; 2]; 2] = [[2, 1], [2, 1]];

/// C `get_msb` (definitions.h:603). Duplicated from `crate::cdef`'s
/// private `get_msb`.
#[inline]
pub(super) fn get_msb_hbd(n: u32) -> i32 {
    debug_assert!(n != 0);
    31 - n.leading_zeros() as i32
}

/// C `constrain` (cdef.c:20). Duplicated from `crate::cdef`'s private
/// `constrain`.
#[inline]
pub(super) fn constrain_hbd(diff: i32, threshold: i32, damping: i32) -> i32 {
    if threshold == 0 {
        return 0;
    }
    let shift = (damping - get_msb_hbd(threshold as u32)).max(0);
    let sign = if diff < 0 { -1 } else { 1 };
    sign * diff.abs().min((threshold - (diff.abs() >> shift)).max(0))
}

/// `svt_cdef_filter_block_c` (cdef.c:193-254), `dst16` arm: identical
/// arithmetic to `crate::cdef::cdef_filter_block` (the `dst8` arm), storing
/// into a `u16` output instead. See the section doc — this is a pure
/// store-type variant, not a new algorithm.
#[allow(clippy::too_many_arguments)]
pub fn cdef_filter_block_hbd(
    dst: &mut [u16],
    doff: usize,
    dstride: usize,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) {
    incant!(
        cdef_filter_block_hbd_impl(
            dst,
            doff,
            dstride,
            inb,
            ioff,
            pri_strength,
            sec_strength,
            dir,
            pri_damping,
            sec_damping,
            bsize,
            coeff_shift,
            subsampling_factor
        ),
        [v3, neon, scalar]
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn cdef_filter_block_hbd_impl_scalar(
    _token: ScalarToken,
    dst: &mut [u16],
    doff: usize,
    dstride: usize,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) {
    cdef_filter_block_hbd_core(
        dst,
        doff,
        dstride,
        inb,
        ioff,
        pri_strength,
        sec_strength,
        dir,
        pri_damping,
        sec_damping,
        bsize,
        coeff_shift,
        subsampling_factor,
    );
}

/// NEON dst16 CDEF filter.
///
/// Mirrors the AVX2 arm exactly, and shares its column kernel: the filtered
/// values are produced by `cdef::cdef_filter_cols8_neon` — already proven
/// byte-identical to C by `tests/c_parity_cdef.rs` — and this differs from the
/// dst8 arm only in the output cast (`as u16` rather than `as u8`).
///
/// Only the `cols == 8` shapes take the vector path; the 4-wide chroma shapes
/// fall back to the scalar core, same as AVX2.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
pub(super) fn cdef_filter_block_hbd_impl_neon(
    token: NeonToken,
    dst: &mut [u16],
    doff: usize,
    dstride: usize,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) {
    let cols = if bsize == BLOCK_8X8 || bsize == BLOCK_8X4 {
        8
    } else {
        4
    };
    if cols != 8 {
        cdef_filter_block_hbd_core(
            dst,
            doff,
            dstride,
            inb,
            ioff,
            pri_strength,
            sec_strength,
            dir,
            pri_damping,
            sec_damping,
            bsize,
            coeff_shift,
            subsampling_factor,
        );
        return;
    }
    let rows = if bsize == BLOCK_8X8 || bsize == BLOCK_4X8 {
        8
    } else {
        4
    };
    let mut scratch = [0i32; 64];
    crate::cdef::cdef_filter_cols8_neon(
        token,
        inb,
        ioff,
        pri_strength,
        sec_strength,
        dir,
        pri_damping,
        sec_damping,
        coeff_shift,
        rows,
        subsampling_factor as i32,
        &mut scratch,
    );
    let mut i = 0i32;
    while i < rows {
        let drow = doff + i as usize * dstride;
        let srow = i as usize * 8;
        for j in 0..8usize {
            dst[drow + j] = scratch[srow + j] as u16;
        }
        i += subsampling_factor as i32;
    }
}

/// AVX2 dst16 filter — the bd10/bd12 CDEF search's per-block filter. Byte-identical
/// to [`cdef_filter_block_hbd_core`]; reuses the shared 8-lane compute
/// ([`crate::cdef::cdef_filter_cols8_v3`]) since the dst16 arm differs from dst8
/// only in the output store type (`as u16` vs `as u8`).
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
pub(super) fn cdef_filter_block_hbd_impl_v3(
    token: Desktop64,
    dst: &mut [u16],
    doff: usize,
    dstride: usize,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) {
    let cols = if bsize == BLOCK_8X8 || bsize == BLOCK_8X4 {
        8
    } else {
        4
    };
    if cols != 8 {
        cdef_filter_block_hbd_core(
            dst,
            doff,
            dstride,
            inb,
            ioff,
            pri_strength,
            sec_strength,
            dir,
            pri_damping,
            sec_damping,
            bsize,
            coeff_shift,
            subsampling_factor,
        );
        return;
    }
    let rows = if bsize == BLOCK_8X8 || bsize == BLOCK_4X8 {
        8
    } else {
        4
    };
    let mut scratch = [0i32; 64];
    crate::cdef::cdef_filter_cols8_v3(
        token,
        inb,
        ioff,
        pri_strength,
        sec_strength,
        dir,
        pri_damping,
        sec_damping,
        coeff_shift,
        rows,
        subsampling_factor as i32,
        &mut scratch,
    );
    let mut i = 0i32;
    while i < rows {
        let drow = doff + i as usize * dstride;
        let srow = i as usize * 8;
        for j in 0..8usize {
            dst[drow + j] = scratch[srow + j] as u16;
        }
        i += subsampling_factor as i32;
    }
}

/// Scalar reference body for [`cdef_filter_block_hbd`] (`svt_cdef_filter_block_c`
/// dst16 arm). The AVX2 path is proven byte-identical to this against real C in
/// `tests/c_parity_cdef.rs`.
#[allow(clippy::too_many_arguments)]
pub(super) fn cdef_filter_block_hbd_core(
    dst: &mut [u16],
    doff: usize,
    dstride: usize,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: usize,
) {
    let s = CDEF_BSTRIDE as i32;
    let pri_taps = CDEF_PRI_TAPS_HBD[((pri_strength >> coeff_shift) & 1) as usize];
    let sec_taps = CDEF_SEC_TAPS_HBD[((pri_strength >> coeff_shift) & 1) as usize];
    let rows = if bsize == BLOCK_8X8 || bsize == BLOCK_4X8 {
        8
    } else {
        4
    };
    let cols = if bsize == BLOCK_8X8 || bsize == BLOCK_8X4 {
        8
    } else {
        4
    };

    let at = |i: i32, j: i32, off: i32| -> u16 { inb[(ioff as i32 + i * s + j + off) as usize] };

    let mut i = 0i32;
    while i < rows {
        for j in 0..cols {
            let mut sum = 0i16;
            let x = at(i, j, 0) as i16;
            let mut max = x as i32;
            let mut min = x as i32;
            for k in 0..2usize {
                let p0 = at(i, j, cdef_direction_hbd(dir, k)) as i16;
                let p1 = at(i, j, -cdef_direction_hbd(dir, k)) as i16;
                sum = sum.wrapping_add(
                    (pri_taps[k] * constrain_hbd(p0 as i32 - x as i32, pri_strength, pri_damping))
                        as i16,
                );
                sum = sum.wrapping_add(
                    (pri_taps[k] * constrain_hbd(p1 as i32 - x as i32, pri_strength, pri_damping))
                        as i16,
                );
                if p0 as u16 != CDEF_VERY_LARGE {
                    max = (p0 as i32).max(max);
                }
                if p1 as u16 != CDEF_VERY_LARGE {
                    max = (p1 as i32).max(max);
                }
                min = (p0 as i32).min(min);
                min = (p1 as i32).min(min);
                let s0 = at(i, j, cdef_direction_hbd(dir + 2, k)) as i16;
                let s1 = at(i, j, -cdef_direction_hbd(dir + 2, k)) as i16;
                let s2 = at(i, j, cdef_direction_hbd(dir - 2, k)) as i16;
                let s3 = at(i, j, -cdef_direction_hbd(dir - 2, k)) as i16;
                if s0 as u16 != CDEF_VERY_LARGE {
                    max = (s0 as i32).max(max);
                }
                if s1 as u16 != CDEF_VERY_LARGE {
                    max = (s1 as i32).max(max);
                }
                if s2 as u16 != CDEF_VERY_LARGE {
                    max = (s2 as i32).max(max);
                }
                if s3 as u16 != CDEF_VERY_LARGE {
                    max = (s3 as i32).max(max);
                }
                min = (s0 as i32).min(min);
                min = (s1 as i32).min(min);
                min = (s2 as i32).min(min);
                min = (s3 as i32).min(min);
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain_hbd(s0 as i32 - x as i32, sec_strength, sec_damping))
                        as i16,
                );
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain_hbd(s1 as i32 - x as i32, sec_strength, sec_damping))
                        as i16,
                );
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain_hbd(s2 as i32 - x as i32, sec_strength, sec_damping))
                        as i16,
                );
                sum = sum.wrapping_add(
                    (sec_taps[k] * constrain_hbd(s3 as i32 - x as i32, sec_strength, sec_damping))
                        as i16,
                );
            }
            let y = (x as i32 + ((8 + sum as i32 - i32::from(sum < 0)) >> 4)).clamp(min, max);
            dst[doff + i as usize * dstride + j as usize] = y as u16;
        }
        i += subsampling_factor as i32;
    }
}

// =============================================================================
// 9. Quant: dc/ac_quant_qtx bit-depth switch shape.
// C: inv_transforms.c:3462-3490 (`svt_aom_dc_quant_qtx`, `svt_aom_ac_
// quant_qtx`), `MAXQ` = definitions.h:1658.
//
// The 256-entry bd10/bd12 qlookup table VALUES are intentionally NOT
// transcribed here (docs/bd10-port-map.md: generate via
// `xtask/transcribe_bd10_qlookup.py`, NOT run by this translation pass) —
// `dc_qlookup_10`/`ac_qlookup_10`/`_12` below are `unimplemented!()`
// placeholders, mirroring the existing pattern in
// `svtav1_encoder::bd10::{dc_qlookup_10, ac_qlookup_10}` (a SEPARATE crate;
// cannot be reused directly, hence this file's own placeholder copies).
//
// The zbin factor (`svt_aom_get_qzbin_factor`, inv_transforms.c:3492-3505)
// is intentionally NOT duplicated — task scope: "qzbin factor already in
// bd10.rs — cross-reference, don't duplicate." See the correctness finding
// immediately below the placeholders.
//
// NOTE on `dc_quant_qtx`/`ac_quant_qtx`: the SWITCH SHAPE is a faithful,
// complete translation of C. The bd10 tables it dispatches to ARE
// transcribed and FFI-verified against real C — but in the ENCODER crate
// (`svtav1_encoder::bd10::{DC,AC}_QLOOKUP_10`, tests/c_parity_bd10_quant.rs),
// not here: this DSP crate cannot depend on the encoder crate. The bd10/bd12
// arms below are still `unimplemented!()` placeholders; wiring them means
// sharing the encoder tables (or relocating them to a common crate) — a
// tracked de-duplication, NOT a re-transcription.
// =============================================================================

/// C `MAXQ` (definitions.h:1658).
pub(super) const MAXQ: i32 = 255;

/// C `svt_aom_dc_quant_qtx` (inv_transforms.c:3462-3475): bit-depth switch
/// dispatching to the per-bd dc qlookup table.
pub fn dc_quant_qtx(qindex: i32, delta: i32, bd: u8) -> i16 {
    let q_clamped = (qindex + delta).clamp(0, MAXQ) as usize;
    match bd {
        8 => DC_QLOOKUP_8[q_clamped],
        10 => dc_qlookup_10(q_clamped as u8),
        12 => dc_qlookup_12(q_clamped as u8),
        _ => unreachable!("bit_depth should be 8, 10, or 12 (inv_transforms.c:3471-3472 assert)"),
    }
}

/// C `svt_aom_ac_quant_qtx` (inv_transforms.c:3477-3490): bit-depth switch
/// dispatching to the per-bd ac qlookup table.
pub fn ac_quant_qtx(qindex: i32, delta: i32, bd: u8) -> i16 {
    let q_clamped = (qindex + delta).clamp(0, MAXQ) as usize;
    match bd {
        8 => AC_QLOOKUP_8[q_clamped],
        10 => ac_qlookup_10(q_clamped as u8),
        12 => ac_qlookup_12(q_clamped as u8),
        _ => unreachable!("bit_depth should be 8, 10, or 12 (inv_transforms.c:3486-3487 assert)"),
    }
}

/// C `dc_qlookup_10_QTX` (inv_transforms.c:3425-3459), 256 entries.
///
/// The transcribed body lives (and is FFI-verified) at
/// `svtav1_encoder::bd10::DC_QLOOKUP_10` — this DSP crate cannot depend on the
/// encoder crate, so this placeholder stays until the tables are relocated to
/// a shared crate. Wire, don't re-transcribe.
pub fn dc_qlookup_10(_qindex: u8) -> i16 {
    unimplemented!("bd10 DC qlookup lives in svtav1_encoder::bd10 (FFI-verified); share it here")
}

/// C `ac_qlookup_10_QTX` (inv_transforms.c:3373-3423), 256 entries.
///
/// See [`dc_qlookup_10`] — the FFI-verified body is
/// `svtav1_encoder::bd10::AC_QLOOKUP_10`.
pub fn ac_qlookup_10(_qindex: u8) -> i16 {
    unimplemented!("bd10 AC qlookup lives in svtav1_encoder::bd10 (FFI-verified); share it here")
}

/// bd12 is OUT OF SCOPE for this port (docs/bd10-port-map.md: "bd 8 or 10
/// only"); kept only so `dc_quant_qtx`'s switch shape matches C's real
/// 3-arm dispatch. PORT-NOTE(unverified): never intended to be transcribed
/// under this task.
pub fn dc_qlookup_12(_qindex: u8) -> i16 {
    unimplemented!("bd12 out of scope per docs/bd10-port-map.md")
}

/// See [`dc_qlookup_12`].
pub fn ac_qlookup_12(_qindex: u8) -> i16 {
    unimplemented!("bd12 out of scope per docs/bd10-port-map.md")
}

// -----------------------------------------------------------------------------
// Correctness finding (cross-check, NOT fixed here — out of this file's
// scope; `svtav1_encoder::bd10` is a sibling crate this translation pass
// does not touch):
//
// `svtav1_encoder::bd10::qzbin_factor(dc_quant_q3, bd)`'s `else` arm
// returns `64`, but C's actual else arm returns `80`
// (`svt_aom_get_qzbin_factor`, inv_transforms.c:3492-3505: `quant < 148 ?
// 84 : 80` for bd8, and the analogous `84 : 80` shape for bd10/bd12 — NOT
// `84 : 64`). C also special-cases `q == 0 -> 64` UNCONDITIONALLY before
// even looking at `quant`; the sibling function's signature has no `q`
// parameter at all, so it structurally cannot reproduce that special case.
// This looks like a real, pre-existing bug in that (already UNWIRED,
// unverified) sibling module. Flagged here because this task's own item 6
// explicitly cross-references that function; NOT corrected in this pass
// (scope is the new `hbd.rs` module only) — see project CLAUDE.md's
// UNWIRED index entry for `bd10.rs` for the tracking note.
// -----------------------------------------------------------------------------

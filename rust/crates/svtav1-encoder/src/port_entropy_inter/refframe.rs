//! Reference-frame signalling: neighbour ref counts, every prediction
//! context, every CDF selector, and `write_ref_frames` itself.
//!
//! C reference: `Source/Lib/Codec/entropy_coding.c`
//! (`svt_aom_collect_neighbors_ref_counts_new` :1877,
//! `svt_aom_get_reference_mode_context_new` :1833,
//! `svt_aom_get_comp_reference_type_context_new` :1695,
//! `svt_av1_get_pred_context_uni_comp_ref_p{,1,2}` :1774/:1797/:1819,
//! the five shared count comparators :1912..:1978,
//! `svt_av1_get_pred_context_{comp_ref,comp_bwdref,single_ref}_*`
//! :1992..:2092, the ten `svt_aom_get_pred_cdf_*` selectors :1650..:2061,
//! and `write_ref_frames` :2098).

use crate::entropy::context::FrameContext;
use crate::entropy::writer::AomWriter;
use crate::port_entropy_inter::{InterCdfs, NeighborMi, Neighbors};
use svtav1_types::block::BlockSize;
use svtav1_types::tables::block::{BLOCK_SIZE_HIGH, BLOCK_SIZE_WIDE};

/// C `INTRA_FRAME` (definitions.h) — reference id 0.
pub const INTRA_FRAME: i8 = 0;
/// C `LAST_FRAME`.
pub const LAST_FRAME: i8 = 1;
/// C `LAST2_FRAME`.
pub const LAST2_FRAME: i8 = 2;
/// C `LAST3_FRAME`.
pub const LAST3_FRAME: i8 = 3;
/// C `GOLDEN_FRAME`.
pub const GOLDEN_FRAME: i8 = 4;
/// C `BWDREF_FRAME`.
pub const BWDREF_FRAME: i8 = 5;
/// C `ALTREF2_FRAME`.
pub const ALTREF2_FRAME: i8 = 6;
/// C `ALTREF_FRAME`.
pub const ALTREF_FRAME: i8 = 7;
/// C `TOTAL_REFS_PER_FRAME` — the width of `xd->neighbors_ref_counts`.
pub const TOTAL_REFS_PER_FRAME: usize = 8;

/// C `CHECK_BACKWARD_REFS` (inter_prediction.h:279) — note it is a RANGE
/// check, `>= BWDREF && <= ALTREF`, not just `>= BWDREF`.
#[inline]
pub const fn is_backward_ref_frame(rf: i8) -> bool {
    rf >= BWDREF_FRAME && rf <= ALTREF_FRAME
}

/// C `is_comp_ref_allowed` (inter_prediction.h:284).
#[inline]
pub fn is_comp_ref_allowed(bsize: BlockSize) -> bool {
    let i = bsize.as_index();
    BLOCK_SIZE_WIDE[i].min(BLOCK_SIZE_HIGH[i]) >= 8
}

/// C `CompReferenceType` (definitions.h).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompReferenceType {
    /// `UNIDIR_COMP_REFERENCE` — both refs on the same side.
    Unidir = 0,
    /// `BIDIR_COMP_REFERENCE`.
    Bidir = 1,
}

/// C `frm_hdr->reference_mode` (definitions.h `ReferenceMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceMode {
    /// `SINGLE_REFERENCE` — no block may be compound.
    Single = 0,
    /// `COMPOUND_REFERENCE` — every block is compound.
    Compound = 1,
    /// `REFERENCE_MODE_SELECT` — the `comp_inter` flag is coded per block.
    Select = 2,
}

// ---- 1. neighbour reference counts ----

/// C `svt_aom_collect_neighbors_ref_counts_new` (entropy_coding.c:1877).
///
/// Must run BEFORE any of the ref-frame contexts below: they all read
/// `xd->neighbors_ref_counts`, which is zeroed and repopulated here. Skipping
/// it leaves every count at 0, which is a *valid-looking* context of 1 for
/// every bit — so the desync is silent, not a panic.
///
/// Note the gates are `up_available` / `left_available` (NOT the `above_mbmi`
/// pointer), and that a neighbour contributes its second ref only when
/// `has_second_ref` holds.
pub fn collect_neighbors_ref_counts(nb: &Neighbors) -> [u8; TOTAL_REFS_PER_FRAME] {
    let mut counts = [0u8; TOTAL_REFS_PER_FRAME];
    let mut add = |mi: &NeighborMi| {
        if !mi.is_inter_block() {
            return;
        }
        counts[mi.ref_frame[0].clamp(0, 7) as usize] += 1;
        if mi.has_second_ref() {
            counts[mi.ref_frame[1].clamp(0, 7) as usize] += 1;
        }
    };
    if let Some(a) = nb.above_avail() {
        add(a);
    }
    if let Some(l) = nb.left_avail() {
        add(l);
    }
    counts
}

// ---- 2. the five shared count comparators ----
//
// Each is C's `(x == y) ? 1 : (x < y ? 0 : 2)` vote over two count groups.

#[inline]
fn vote(a: u32, b: u32) -> usize {
    if a == b {
        1
    } else if a < b {
        0
    } else {
        2
    }
}

#[inline]
fn c(counts: &[u8; TOTAL_REFS_PER_FRAME], rf: i8) -> u32 {
    counts[rf as usize] as u32
}

/// C `get_pred_context_ll2_or_l3gld` (entropy_coding.c:1912) — {LAST,LAST2}
/// against {LAST3,GOLDEN}.
pub fn pred_context_ll2_or_l3gld(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    vote(
        c(counts, LAST_FRAME) + c(counts, LAST2_FRAME),
        c(counts, LAST3_FRAME) + c(counts, GOLDEN_FRAME),
    )
}

/// C `get_pred_context_last_or_last2` (entropy_coding.c:1928).
pub fn pred_context_last_or_last2(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    vote(c(counts, LAST_FRAME), c(counts, LAST2_FRAME))
}

/// C `get_pred_context_last3_or_gld` (entropy_coding.c:1943).
pub fn pred_context_last3_or_gld(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    vote(c(counts, LAST3_FRAME), c(counts, GOLDEN_FRAME))
}

/// C `get_pred_context_brfarf2_or_arf` (entropy_coding.c:1959) —
/// {BWDREF,ALTREF2} against ALTREF.
pub fn pred_context_brfarf2_or_arf(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    vote(
        c(counts, BWDREF_FRAME) + c(counts, ALTREF2_FRAME),
        c(counts, ALTREF_FRAME),
    )
}

/// C `get_pred_context_brf_or_arf2` (entropy_coding.c:1973).
pub fn pred_context_brf_or_arf2(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    vote(c(counts, BWDREF_FRAME), c(counts, ALTREF2_FRAME))
}

// ---- 3. single-ref contexts (entropy_coding.c:2026..:2092) ----

/// C `svt_av1_get_pred_context_single_ref_p1` (:2026) — forward vs backward.
pub fn pred_context_single_ref_p1(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    let fwd = c(counts, LAST_FRAME)
        + c(counts, LAST2_FRAME)
        + c(counts, LAST3_FRAME)
        + c(counts, GOLDEN_FRAME);
    let bwd = c(counts, BWDREF_FRAME) + c(counts, ALTREF2_FRAME) + c(counts, ALTREF_FRAME);
    vote(fwd, bwd)
}

/// C `svt_av1_get_pred_context_single_ref_p2` (:2068) — ALTREF vs
/// {ALTREF2,BWDREF}.
pub fn pred_context_single_ref_p2(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    pred_context_brfarf2_or_arf(counts)
}

/// C `svt_av1_get_pred_context_single_ref_p3` (:2074) — {LAST3,GOLDEN} vs
/// {LAST,LAST2}.
pub fn pred_context_single_ref_p3(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    pred_context_ll2_or_l3gld(counts)
}

/// C `svt_av1_get_pred_context_single_ref_p4` (:2080) — LAST vs LAST2.
pub fn pred_context_single_ref_p4(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    pred_context_last_or_last2(counts)
}

/// C `svt_av1_get_pred_context_single_ref_p5` (:2086) — LAST3 vs GOLDEN.
pub fn pred_context_single_ref_p5(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    pred_context_last3_or_gld(counts)
}

/// C `svt_av1_get_pred_context_single_ref_p6` (:2092) — BWDREF vs ALTREF2.
pub fn pred_context_single_ref_p6(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    pred_context_brf_or_arf2(counts)
}

// ---- 4. compound-ref contexts (entropy_coding.c:1774..:2018) ----

/// C `svt_av1_get_pred_context_comp_ref_p` (:1992).
pub fn pred_context_comp_ref_p(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    pred_context_ll2_or_l3gld(counts)
}

/// C `svt_av1_get_pred_context_comp_ref_p1` (:1999).
pub fn pred_context_comp_ref_p1(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    pred_context_last_or_last2(counts)
}

/// C `svt_av1_get_pred_context_comp_ref_p2` (:2006).
pub fn pred_context_comp_ref_p2(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    pred_context_last3_or_gld(counts)
}

/// C `svt_av1_get_pred_context_comp_bwdref_p` (:2012).
pub fn pred_context_comp_bwdref_p(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    pred_context_brfarf2_or_arf(counts)
}

/// C `svt_av1_get_pred_context_comp_bwdref_p1` (:2018).
pub fn pred_context_comp_bwdref_p1(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    pred_context_brf_or_arf2(counts)
}

/// C `svt_av1_get_pred_context_uni_comp_ref_p` (:1774) — forward count vs
/// backward count.
pub fn pred_context_uni_comp_ref_p(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    let frf = c(counts, LAST_FRAME)
        + c(counts, LAST2_FRAME)
        + c(counts, LAST3_FRAME)
        + c(counts, GOLDEN_FRAME);
    let brf = c(counts, BWDREF_FRAME) + c(counts, ALTREF2_FRAME) + c(counts, ALTREF_FRAME);
    vote(frf, brf)
}

/// C `svt_av1_get_pred_context_uni_comp_ref_p1` (:1797) — LAST2 vs
/// {LAST3,GOLDEN}.
pub fn pred_context_uni_comp_ref_p1(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    vote(
        c(counts, LAST2_FRAME),
        c(counts, LAST3_FRAME) + c(counts, GOLDEN_FRAME),
    )
}

/// C `svt_av1_get_pred_context_uni_comp_ref_p2` (:1819) — LAST3 vs GOLDEN.
pub fn pred_context_uni_comp_ref_p2(counts: &[u8; TOTAL_REFS_PER_FRAME]) -> usize {
    vote(c(counts, LAST3_FRAME), c(counts, GOLDEN_FRAME))
}

// ---- 5. neighbour-shape contexts (no ref counts) ----

/// C `svt_aom_get_reference_mode_context_new` (entropy_coding.c:1833) — the
/// context for the `comp_inter` (single-vs-compound) flag.
pub fn reference_mode_context(nb: &Neighbors) -> usize {
    match (nb.above_avail(), nb.left_avail()) {
        (Some(a), Some(l)) => {
            if !a.has_second_ref() && !l.has_second_ref() {
                usize::from(
                    is_backward_ref_frame(a.ref_frame[0]) ^ is_backward_ref_frame(l.ref_frame[0]),
                )
            } else if !a.has_second_ref() {
                2 + usize::from(is_backward_ref_frame(a.ref_frame[0]) || !a.is_inter_block())
            } else if !l.has_second_ref() {
                2 + usize::from(is_backward_ref_frame(l.ref_frame[0]) || !l.is_inter_block())
            } else {
                4
            }
        }
        (Some(e), None) | (None, Some(e)) => {
            if !e.has_second_ref() {
                usize::from(is_backward_ref_frame(e.ref_frame[0]))
            } else {
                3
            }
        }
        (None, None) => 1,
    }
}

/// C `svt_aom_get_comp_reference_type_context_new` (entropy_coding.c:1695) —
/// the context for the UNIDIR-vs-BIDIR compound-reference-type symbol.
pub fn comp_reference_type_context(nb: &Neighbors) -> usize {
    match (nb.above_avail(), nb.left_avail()) {
        (Some(a), Some(l)) => {
            let above_intra = !a.is_inter_block();
            let left_intra = !l.is_inter_block();
            if above_intra && left_intra {
                2
            } else if above_intra || left_intra {
                let inter = if above_intra { l } else { a };
                if !inter.has_second_ref() {
                    2
                } else {
                    1 + 2 * usize::from(inter.has_uni_comp_refs())
                }
            } else {
                let a_sg = !a.has_second_ref();
                let l_sg = !l.has_second_ref();
                let frfa = a.ref_frame[0];
                let frfl = l.ref_frame[0];
                if a_sg && l_sg {
                    1 + 2 * usize::from(
                        !(is_backward_ref_frame(frfa) ^ is_backward_ref_frame(frfl)),
                    )
                } else if l_sg || a_sg {
                    let uni_rfc = if a_sg {
                        l.has_uni_comp_refs()
                    } else {
                        a.has_uni_comp_refs()
                    };
                    if !uni_rfc {
                        1
                    } else {
                        3 + usize::from(
                            !(is_backward_ref_frame(frfa) ^ is_backward_ref_frame(frfl)),
                        )
                    }
                } else {
                    let a_uni = a.has_uni_comp_refs();
                    let l_uni = l.has_uni_comp_refs();
                    if !a_uni && !l_uni {
                        0
                    } else if !a_uni || !l_uni {
                        2
                    } else {
                        3 + usize::from(!((frfa == BWDREF_FRAME) ^ (frfl == BWDREF_FRAME)))
                    }
                }
            }
        }
        (Some(e), None) | (None, Some(e)) => {
            if !e.is_inter_block() || !e.has_second_ref() {
                2
            } else {
                4 * usize::from(e.has_uni_comp_refs())
            }
        }
        (None, None) => 2,
    }
}

// ---- 6. CDF selectors ----
//
// C's `svt_aom_get_pred_cdf_*` return a POINTER into a `[ctx][slot]` table;
// the port returns the pair, because a `&mut` into an array field cannot be
// handed out and then used alongside the writer. `write_ref_frames` below is
// the only consumer, and `tests/c_parity_entropy_inter.rs` gates the pair
// against the flat row index C's pointer resolves to.

/// C `svt_aom_get_reference_mode_cdf` (entropy_coding.c:1636): the CDF is
/// `comp_inter_cdf[ctx]`, so the selector reduces to the context — the table
/// is fixed and there is no slot. Kept as a named function because C has one
/// and `write_ref_frames` dispatches through it.
#[inline]
pub fn pred_cdf_reference_mode(nb: &Neighbors) -> usize {
    reference_mode_context(nb)
}

/// C `svt_aom_get_comp_reference_type_cdf` (entropy_coding.c:1650): the CDF is
/// `comp_ref_type_cdf[ctx]` — same shape as
/// [`pred_cdf_reference_mode`], one row per context, no slot.
#[inline]
pub fn pred_cdf_comp_reference_type(nb: &Neighbors) -> usize {
    comp_reference_type_context(nb)
}

/// C `svt_aom_get_pred_cdf_single_ref_p{n}` (:2041..:2061): the CDF is
/// `single_ref_cdf[ctx(n)][n - 1]`.
pub fn pred_cdf_single_ref(counts: &[u8; TOTAL_REFS_PER_FRAME], n: usize) -> (usize, usize) {
    let ctx = match n {
        1 => pred_context_single_ref_p1(counts),
        2 => pred_context_single_ref_p2(counts),
        3 => pred_context_single_ref_p3(counts),
        4 => pred_context_single_ref_p4(counts),
        5 => pred_context_single_ref_p5(counts),
        6 => pred_context_single_ref_p6(counts),
        _ => unreachable!("single_ref bit index {n} is outside 1..=6"),
    };
    (ctx, n - 1)
}

/// C `svt_aom_get_pred_cdf_comp_ref_p{,1,2}` (:1670..:1680): the CDF is
/// `comp_ref_cdf[ctx(n)][n]`, `n` in `0..=2`.
pub fn pred_cdf_comp_ref(counts: &[u8; TOTAL_REFS_PER_FRAME], n: usize) -> (usize, usize) {
    let ctx = match n {
        0 => pred_context_comp_ref_p(counts),
        1 => pred_context_comp_ref_p1(counts),
        2 => pred_context_comp_ref_p2(counts),
        _ => unreachable!("comp_ref bit index {n} is outside 0..=2"),
    };
    (ctx, n)
}

/// C `svt_aom_get_pred_cdf_comp_bwdref_p{,1}` (:1685/:1690): the CDF is
/// `comp_bwdref_cdf[ctx(n)][n]`, `n` in `0..=1`.
pub fn pred_cdf_comp_bwdref(counts: &[u8; TOTAL_REFS_PER_FRAME], n: usize) -> (usize, usize) {
    let ctx = match n {
        0 => pred_context_comp_bwdref_p(counts),
        1 => pred_context_comp_bwdref_p1(counts),
        _ => unreachable!("comp_bwdref bit index {n} is outside 0..=1"),
    };
    (ctx, n)
}

/// C `svt_aom_get_pred_cdf_uni_comp_ref_p{,1,2}` (:1655..:1665): the CDF is
/// `uni_comp_ref_cdf[ctx(n)][n]`, `n` in `0..=2`.
pub fn pred_cdf_uni_comp_ref(counts: &[u8; TOTAL_REFS_PER_FRAME], n: usize) -> (usize, usize) {
    let ctx = match n {
        0 => pred_context_uni_comp_ref_p(counts),
        1 => pred_context_uni_comp_ref_p1(counts),
        2 => pred_context_uni_comp_ref_p2(counts),
        _ => unreachable!("uni_comp_ref bit index {n} is outside 0..=2"),
    };
    (ctx, n)
}

// ---- 7. write_ref_frames (entropy_coding.c:2098) ----

/// The block fields `write_ref_frames` reads off `xd->mi[0]`.
#[derive(Clone, Copy, Debug)]
pub struct RefFrameBlock {
    /// C `mbmi->block_mi.ref_frame`.
    pub ref_frame: [i8; 2],
    /// C `mbmi->bsize` — only used by the `is_comp_ref_allowed` gate.
    pub bsize: BlockSize,
}

impl RefFrameBlock {
    /// C `has_second_ref` (block_structures.h:105).
    #[inline]
    pub fn has_second_ref(&self) -> bool {
        self.ref_frame[1] > INTRA_FRAME
    }
    /// C `has_uni_comp_refs` (block_structures.h:109).
    #[inline]
    pub fn has_uni_comp_refs(&self) -> bool {
        self.has_second_ref()
            && !((self.ref_frame[0] >= BWDREF_FRAME) ^ (self.ref_frame[1] >= BWDREF_FRAME))
    }
}

/// C `write_ref_frames` (entropy_coding.c:2098) — step 2 of the inter block
/// walk (`inter_mv_code.rs`'s recorded order).
///
/// `counts` MUST come from [`collect_neighbors_ref_counts`] on the same
/// neighbours (C runs `svt_aom_collect_neighbors_ref_counts_new` immediately
/// before this call); passing stale or zeroed counts silently picks the wrong
/// CDF row.
///
/// `fc` supplies the tables that already exist and already hold the real C
/// defaults (`comp_inter_cdf`, `single_ref_cdf`, `comp_ref_cdf`); `ic`
/// supplies the three that `FrameContext` does not have at all
/// (`comp_ref_type`, `uni_comp_ref`, `comp_bwdref`). See
/// [`crate::port_entropy_inter::cdfs`] for why the split exists.
pub fn write_ref_frames(
    w: &mut AomWriter,
    fc: &mut FrameContext,
    ic: &mut InterCdfs,
    nb: &Neighbors,
    counts: &[u8; TOTAL_REFS_PER_FRAME],
    reference_mode: ReferenceMode,
    blk: &RefFrameBlock,
) {
    let is_compound = blk.has_second_ref();

    // C's `else` arm is an assert only: SINGLE_REFERENCE / COMPOUND_REFERENCE
    // code no flag at all.
    if reference_mode == ReferenceMode::Select && is_comp_ref_allowed(blk.bsize) {
        let ctx = pred_cdf_reference_mode(nb);
        w.write_symbol(usize::from(is_compound), &mut fc.comp_inter_cdf[ctx], 2);
    }

    if is_compound {
        let comp_ref_type = if blk.has_uni_comp_refs() {
            CompReferenceType::Unidir
        } else {
            CompReferenceType::Bidir
        };
        let ctx = pred_cdf_comp_reference_type(nb);
        w.write_symbol(comp_ref_type as usize, &mut ic.comp_ref_type_cdf[ctx], 2);

        if comp_ref_type == CompReferenceType::Unidir {
            let bit = blk.ref_frame[0] == BWDREF_FRAME;
            let (c0, s0) = pred_cdf_uni_comp_ref(counts, 0);
            w.write_symbol(usize::from(bit), &mut ic.uni_comp_ref_cdf[c0][s0], 2);
            if !bit {
                let bit1 = blk.ref_frame[1] == LAST3_FRAME || blk.ref_frame[1] == GOLDEN_FRAME;
                let (c1, s1) = pred_cdf_uni_comp_ref(counts, 1);
                w.write_symbol(usize::from(bit1), &mut ic.uni_comp_ref_cdf[c1][s1], 2);
                if bit1 {
                    let bit2 = blk.ref_frame[1] == GOLDEN_FRAME;
                    let (c2, s2) = pred_cdf_uni_comp_ref(counts, 2);
                    w.write_symbol(usize::from(bit2), &mut ic.uni_comp_ref_cdf[c2][s2], 2);
                }
            }
            return;
        }

        let bit = blk.ref_frame[0] == GOLDEN_FRAME || blk.ref_frame[0] == LAST3_FRAME;
        let (c0, s0) = pred_cdf_comp_ref(counts, 0);
        w.write_symbol(usize::from(bit), &mut fc.comp_ref_cdf[c0][s0], 2);
        if !bit {
            let bit1 = blk.ref_frame[0] == LAST2_FRAME;
            let (c1, s1) = pred_cdf_comp_ref(counts, 1);
            w.write_symbol(usize::from(bit1), &mut fc.comp_ref_cdf[c1][s1], 2);
        } else {
            let bit2 = blk.ref_frame[0] == GOLDEN_FRAME;
            let (c2, s2) = pred_cdf_comp_ref(counts, 2);
            w.write_symbol(usize::from(bit2), &mut fc.comp_ref_cdf[c2][s2], 2);
        }

        let bit_bwd = blk.ref_frame[1] == ALTREF_FRAME;
        let (cb, sb) = pred_cdf_comp_bwdref(counts, 0);
        w.write_symbol(usize::from(bit_bwd), &mut ic.comp_bwdref_cdf[cb][sb], 2);
        if !bit_bwd {
            let bit = blk.ref_frame[1] == ALTREF2_FRAME;
            let (cb1, sb1) = pred_cdf_comp_bwdref(counts, 1);
            w.write_symbol(usize::from(bit), &mut ic.comp_bwdref_cdf[cb1][sb1], 2);
        }
    } else {
        let bit0 = blk.ref_frame[0] <= ALTREF_FRAME && blk.ref_frame[0] >= BWDREF_FRAME;
        let (c1, s1) = pred_cdf_single_ref(counts, 1);
        w.write_symbol(usize::from(bit0), &mut fc.single_ref_cdf[c1][s1], 2);

        if bit0 {
            let bit1 = blk.ref_frame[0] == ALTREF_FRAME;
            let (c2, s2) = pred_cdf_single_ref(counts, 2);
            w.write_symbol(usize::from(bit1), &mut fc.single_ref_cdf[c2][s2], 2);
            if !bit1 {
                let bit = blk.ref_frame[0] == ALTREF2_FRAME;
                let (c6, s6) = pred_cdf_single_ref(counts, 6);
                w.write_symbol(usize::from(bit), &mut fc.single_ref_cdf[c6][s6], 2);
            }
        } else {
            let bit2 = blk.ref_frame[0] == LAST3_FRAME || blk.ref_frame[0] == GOLDEN_FRAME;
            let (c3, s3) = pred_cdf_single_ref(counts, 3);
            w.write_symbol(usize::from(bit2), &mut fc.single_ref_cdf[c3][s3], 2);
            if !bit2 {
                let bit3 = blk.ref_frame[0] != LAST_FRAME;
                let (c4, s4) = pred_cdf_single_ref(counts, 4);
                w.write_symbol(usize::from(bit3), &mut fc.single_ref_cdf[c4][s4], 2);
            } else {
                let bit4 = blk.ref_frame[0] != LAST3_FRAME;
                let (c5, s5) = pred_cdf_single_ref(counts, 5);
                w.write_symbol(usize::from(bit4), &mut fc.single_ref_cdf[c5][s5], 2);
            }
        }
    }
}

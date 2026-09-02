//! The reference-signalling rate every inter candidate pays —
//! `estimate_ref_frames_num_bits` and the whole prediction-context
//! family it drives.
//!
//! | this module | C |
//! |---|---|
//! | [`NeighborRefCounts::collect`] | `entropy_coding.c:1877-1902` (`svt_aom_collect_neighbors_ref_counts_new`, EXPORTED) |
//! | [`reference_mode_context`] | `entropy_coding.c:1833-1874` (EXPORTED) |
//! | [`comp_reference_type_context`] | `entropy_coding.c:1695-1763` (EXPORTED) |
//! | [`pred_context_ll2_or_l3gld`] etc. (five vote helpers) | `entropy_coding.c:1912-1985` |
//! | [`single_ref_p1`] .. [`single_ref_p6`] | `entropy_coding.c:2026-2110` |
//! | [`comp_ref_p`] / `_p1` / `_p2`, [`comp_bwdref_p`] / `_p1` | `entropy_coding.c:1990-2018` |
//! | [`uni_comp_ref_p`] / `_p1` / `_p2` | `entropy_coding.c:1773-1830` |
//! | [`estimate_ref_frame_type_bits`] | `rd_cost.c:643-777` (EXPORTED) |
//! | [`estimate_ref_frames_num_bits`] | `product_coding_loop.c:8032-8063` |
//!
//! # Why this matters
//!
//! Without it every inter candidate's rate is short by the whole
//! reference-frame signalling cost — a per-candidate constant that
//! differs BETWEEN candidates, so it reorders RD rather than shifting it
//! uniformly. `estimate_ref_frames_num_bits` precomputes it once per
//! block for every reference pair in `ctx->ref_frame_type_arr`, and both
//! the fast and full inter cost read the result.
//!
//! # Evidence
//!
//! **Tier 1** — `tests/c_parity_md_ref_rate.rs` drives the EXPORTED
//! `estimate_ref_frame_type_bits` (rd_cost.c:643) over randomized
//! neighbour configurations and rate tables. That one oracle reaches
//! every context function in this module, because the C function's only
//! inputs beyond the tables are the two neighbours, and each of the
//! sixteen ref-frame types takes a different path through them. The
//! exported `svt_aom_collect_neighbors_ref_counts_new` and
//! `svt_aom_get_reference_mode_context_new` are driven directly as well.
//!
//! `estimate_ref_frames_num_bits` itself is `static`; it is a loop over
//! the reference array calling the tier-1 function, and is **tier 4**.

use svtav1_types::prediction::PredictionMode;

/// C `TOTAL_REFS_PER_FRAME` (definitions.h:1398).
pub const TOTAL_REFS_PER_FRAME: usize = 8;
/// C `REF_CONTEXTS` / `UNI_COMP_REF_CONTEXTS`.
pub const REF_CONTEXTS: usize = 3;
/// C `COMP_REF_TYPE_CONTEXTS`.
pub const COMP_REF_TYPE_CONTEXTS: usize = 5;
/// C `COMP_INTER_CONTEXTS`.
pub const COMP_INTER_CONTEXTS: usize = 5;

/// C reference-frame ids (definitions.h:1390-1398).
pub const INTRA_FRAME: i8 = 0;
pub const LAST_FRAME: i8 = 1;
pub const LAST2_FRAME: i8 = 2;
pub const LAST3_FRAME: i8 = 3;
pub const GOLDEN_FRAME: i8 = 4;
pub const BWDREF_FRAME: i8 = 5;
pub const ALTREF2_FRAME: i8 = 6;
pub const ALTREF_FRAME: i8 = 7;
/// C `NONE_FRAME`.
pub const NONE_FRAME: i8 = -1;

/// C `BlockModeInfo` as the context functions read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NeighborMi {
    pub ref_frame: [i8; 2],
    pub use_intrabc: bool,
}

impl NeighborMi {
    /// C `has_second_ref` (block_structures.h:106-108): `ref_frame[1] >
    /// INTRA_FRAME`, so `NONE_FRAME` (-1) AND `INTRA_FRAME` (0) both mean
    /// "no second reference".
    #[inline]
    pub fn has_second_ref(&self) -> bool {
        self.ref_frame[1] > INTRA_FRAME
    }

    /// C `is_inter_block` (block_structures.h:119-121): intrabc counts as
    /// inter.
    #[inline]
    pub fn is_inter_block(&self) -> bool {
        self.use_intrabc || self.ref_frame[0] > INTRA_FRAME
    }

    /// C `has_uni_comp_refs` (block_structures.h:110-113): a compound
    /// pair whose two references are on the SAME side of `BWDREF_FRAME`.
    #[inline]
    pub fn has_uni_comp_refs(&self) -> bool {
        self.has_second_ref()
            && !((self.ref_frame[0] >= BWDREF_FRAME) ^ (self.ref_frame[1] >= BWDREF_FRAME))
    }
}

/// C `IS_BACKWARD_REF_FRAME` (inter_prediction.h:279-280).
#[inline]
pub fn is_backward_ref_frame(rf: i8) -> bool {
    (BWDREF_FRAME..=ALTREF_FRAME).contains(&rf)
}

/// C `xd->neighbors_ref_counts` (coding_unit.h:102).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NeighborRefCounts(pub [u8; TOTAL_REFS_PER_FRAME]);

impl NeighborRefCounts {
    /// C `svt_aom_collect_neighbors_ref_counts_new`
    /// (entropy_coding.c:1877-1902, EXPORTED).
    ///
    /// **An INTRA neighbour contributes nothing** — the whole body is
    /// gated on `is_inter_block`. And an intrabc neighbour DOES count,
    /// with `ref_frame[0] == INTRA_FRAME`, so it increments slot 0, which
    /// no context function ever reads.
    pub fn collect(above: Option<NeighborMi>, left: Option<NeighborMi>) -> Self {
        let mut counts = [0u8; TOTAL_REFS_PER_FRAME];
        for mi in [above, left].into_iter().flatten() {
            if !mi.is_inter_block() {
                continue;
            }
            counts[mi.ref_frame[0].max(0) as usize] += 1;
            if mi.has_second_ref() {
                counts[mi.ref_frame[1].max(0) as usize] += 1;
            }
        }
        Self(counts)
    }

    #[inline]
    fn at(&self, rf: i8) -> i32 {
        i32::from(self.0[rf as usize])
    }
}

/// C's three-way vote shared by every count-based context
/// (`a == b ? 1 : a < b ? 0 : 2`).
#[inline]
fn vote(a: i32, b: i32) -> usize {
    if a == b {
        1
    } else if a < b {
        0
    } else {
        2
    }
}

/// C `get_pred_context_ll2_or_l3gld` (entropy_coding.c:1912-1925).
#[inline]
pub fn pred_context_ll2_or_l3gld(c: &NeighborRefCounts) -> usize {
    vote(
        c.at(LAST_FRAME) + c.at(LAST2_FRAME),
        c.at(LAST3_FRAME) + c.at(GOLDEN_FRAME),
    )
}

/// C `get_pred_context_last_or_last2` (entropy_coding.c:1928-1939).
#[inline]
pub fn pred_context_last_or_last2(c: &NeighborRefCounts) -> usize {
    vote(c.at(LAST_FRAME), c.at(LAST2_FRAME))
}

/// C `get_pred_context_last3_or_gld` (entropy_coding.c:1942-1953).
#[inline]
pub fn pred_context_last3_or_gld(c: &NeighborRefCounts) -> usize {
    vote(c.at(LAST3_FRAME), c.at(GOLDEN_FRAME))
}

/// C `get_pred_context_brfarf2_or_arf` (entropy_coding.c:1957-1968).
#[inline]
pub fn pred_context_brfarf2_or_arf(c: &NeighborRefCounts) -> usize {
    vote(c.at(BWDREF_FRAME) + c.at(ALTREF2_FRAME), c.at(ALTREF_FRAME))
}

/// C `get_pred_context_brf_or_arf2` (entropy_coding.c:1971-1983).
#[inline]
pub fn pred_context_brf_or_arf2(c: &NeighborRefCounts) -> usize {
    vote(c.at(BWDREF_FRAME), c.at(ALTREF2_FRAME))
}

/// C `svt_av1_get_pred_context_single_ref_p1` (entropy_coding.c:2026).
#[inline]
pub fn single_ref_p1(c: &NeighborRefCounts) -> usize {
    let fwd = c.at(LAST_FRAME) + c.at(LAST2_FRAME) + c.at(LAST3_FRAME) + c.at(GOLDEN_FRAME);
    let bwd = c.at(BWDREF_FRAME) + c.at(ALTREF2_FRAME) + c.at(ALTREF_FRAME);
    vote(fwd, bwd)
}

/// C `svt_av1_get_pred_context_single_ref_p2` (entropy_coding.c:2070).
#[inline]
pub fn single_ref_p2(c: &NeighborRefCounts) -> usize {
    pred_context_brfarf2_or_arf(c)
}
/// C `svt_av1_get_pred_context_single_ref_p3` (entropy_coding.c:2076).
#[inline]
pub fn single_ref_p3(c: &NeighborRefCounts) -> usize {
    pred_context_ll2_or_l3gld(c)
}
/// C `svt_av1_get_pred_context_single_ref_p4` (entropy_coding.c:2082).
#[inline]
pub fn single_ref_p4(c: &NeighborRefCounts) -> usize {
    pred_context_last_or_last2(c)
}
/// C `svt_av1_get_pred_context_single_ref_p5` (entropy_coding.c:2088).
#[inline]
pub fn single_ref_p5(c: &NeighborRefCounts) -> usize {
    pred_context_last3_or_gld(c)
}
/// C `svt_av1_get_pred_context_single_ref_p6` (entropy_coding.c:2094).
#[inline]
pub fn single_ref_p6(c: &NeighborRefCounts) -> usize {
    pred_context_brf_or_arf2(c)
}

/// C `svt_av1_get_pred_context_comp_ref_p` (entropy_coding.c:1990).
#[inline]
pub fn comp_ref_p(c: &NeighborRefCounts) -> usize {
    pred_context_ll2_or_l3gld(c)
}
/// C `svt_av1_get_pred_context_comp_ref_p1` (entropy_coding.c:1996).
#[inline]
pub fn comp_ref_p1(c: &NeighborRefCounts) -> usize {
    pred_context_last_or_last2(c)
}
/// C `svt_av1_get_pred_context_comp_ref_p2` (entropy_coding.c:2003).
#[inline]
pub fn comp_ref_p2(c: &NeighborRefCounts) -> usize {
    pred_context_last3_or_gld(c)
}
/// C `svt_av1_get_pred_context_comp_bwdref_p` (entropy_coding.c:2009).
#[inline]
pub fn comp_bwdref_p(c: &NeighborRefCounts) -> usize {
    pred_context_brfarf2_or_arf(c)
}
/// C `svt_av1_get_pred_context_comp_bwdref_p1` (entropy_coding.c:2015).
#[inline]
pub fn comp_bwdref_p1(c: &NeighborRefCounts) -> usize {
    pred_context_brf_or_arf2(c)
}

/// C `svt_av1_get_pred_context_uni_comp_ref_p` (entropy_coding.c:1773).
///
/// Textually the same vote as [`single_ref_p1`] — C spells it out twice
/// with different comments and this port keeps both names so each call
/// site cites the function it actually mirrors.
#[inline]
pub fn uni_comp_ref_p(c: &NeighborRefCounts) -> usize {
    let frf = c.at(LAST_FRAME) + c.at(LAST2_FRAME) + c.at(LAST3_FRAME) + c.at(GOLDEN_FRAME);
    let brf = c.at(BWDREF_FRAME) + c.at(ALTREF2_FRAME) + c.at(ALTREF_FRAME);
    vote(frf, brf)
}

/// C `svt_av1_get_pred_context_uni_comp_ref_p1` (entropy_coding.c:1796).
///
/// NOT the same as [`comp_ref_p1`]: this one votes LAST2 against
/// LAST3+GOLDEN, where `comp_ref_p1` votes LAST against LAST2.
#[inline]
pub fn uni_comp_ref_p1(c: &NeighborRefCounts) -> usize {
    vote(c.at(LAST2_FRAME), c.at(LAST3_FRAME) + c.at(GOLDEN_FRAME))
}

/// C `svt_av1_get_pred_context_uni_comp_ref_p2` (entropy_coding.c:1818).
#[inline]
pub fn uni_comp_ref_p2(c: &NeighborRefCounts) -> usize {
    pred_context_last3_or_gld(c)
}

/// C `svt_aom_get_reference_mode_context_new`
/// (entropy_coding.c:1833-1874, EXPORTED).
///
/// Note the asymmetry in the "one of two edges uses comp pred" arms: the
/// `+` term is `IS_BACKWARD_REF_FRAME(rf0) || !is_inter_block(mi)`, so an
/// INTRA neighbour lands in context 3, not 2.
pub fn reference_mode_context(above: Option<NeighborMi>, left: Option<NeighborMi>) -> usize {
    match (above, left) {
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

/// C `svt_aom_get_comp_reference_type_context_new`
/// (entropy_coding.c:1695-1763, EXPORTED).
///
/// Five contexts over a nine-way case split. The two `3 + (!(a ^ b))`
/// terms are the ones easiest to get wrong: they are `3 + (a == b)`, and
/// the comparison is between the two neighbours' FIRST references —
/// backward-ness in the single/comp arm, and `== BWDREF_FRAME` exactly in
/// the unidir/unidir arm.
pub fn comp_reference_type_context(above: Option<NeighborMi>, left: Option<NeighborMi>) -> usize {
    match (above, left) {
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
            // C writes the intra and single-pred cases as two separate
            // `pred_context = 2` assignments; they are kept merged here
            // because the value is identical and the branch order has no
            // side effects.
            if !e.is_inter_block() || !e.has_second_ref() {
                2
            } else {
                4 * usize::from(e.has_uni_comp_refs())
            }
        }
        (None, None) => 2,
    }
}

/// C `MdRateEstimationContext`'s reference-signalling tables, in C's
/// shapes.
#[derive(Debug, Clone)]
pub struct RefFrameFacBits {
    /// C `comp_inter_fac_bits[COMP_INTER_CONTEXTS][2]`. All six tables
    /// are `int32_t` in C and accumulate into a `uint64_t`, so the port
    /// keeps them `i32` and widens at the add — a rate is never
    /// negative, but the widening is C's and not the port's choice.
    pub comp_inter: [[i32; 2]; COMP_INTER_CONTEXTS],
    /// C `comp_ref_type_fac_bits[COMP_REF_TYPE_CONTEXTS][2]`.
    pub comp_ref_type: [[i32; 2]; COMP_REF_TYPE_CONTEXTS],
    /// C `uni_comp_ref_fac_bits[REF_CONTEXTS][UNIDIR_COMP_REFS-1][2]`.
    pub uni_comp_ref: [[[i32; 2]; 3]; REF_CONTEXTS],
    /// C `comp_ref_fac_bits[REF_CONTEXTS][FWD_REFS-1][2]`.
    pub comp_ref: [[[i32; 2]; 3]; REF_CONTEXTS],
    /// C `comp_bwd_ref_fac_bits[REF_CONTEXTS][BWD_REFS-1][2]`.
    pub comp_bwd_ref: [[[i32; 2]; 2]; REF_CONTEXTS],
    /// C `single_ref_fac_bits[REF_CONTEXTS][SINGLE_REFS-1][2]`.
    pub single_ref: [[[i32; 2]; 6]; REF_CONTEXTS],
}

impl RefFrameFacBits {
    /// Fill every table from the LIVE frame contexts, as C's
    /// `svt_aom_estimate_syntax_rate` does.
    ///
    /// Three of the six rows come from
    /// [`crate::port_entropy_inter::InterCdfs`] rather than
    /// [`crate::entropy::context::FrameContext`], for the same reason
    /// [`crate::port_rd_cost::inter_cost::InterFacBits::from_cdfs`] does:
    /// `FrameContext` simply has no `comp_ref_type` / `uni_comp_ref` /
    /// `comp_bwdref` field, and pricing against something else would be a
    /// different rate, not an approximation of the same one.
    #[must_use]
    pub fn from_cdfs(
        fc: &crate::entropy::context::FrameContext,
        ic: &crate::port_entropy_inter::InterCdfs,
    ) -> Self {
        fn fill<const N: usize>(cdf: &[u16]) -> [i32; N] {
            let mut out = [0i32; N];
            crate::quant::syntax_rate_from_cdf(&mut out, cdf);
            out
        }
        Self {
            comp_inter: core::array::from_fn(|i| fill::<2>(&fc.comp_inter_cdf[i])),
            comp_ref_type: core::array::from_fn(|i| fill::<2>(&ic.comp_ref_type_cdf[i])),
            uni_comp_ref: core::array::from_fn(|i| {
                core::array::from_fn(|j| fill::<2>(&ic.uni_comp_ref_cdf[i][j]))
            }),
            comp_ref: core::array::from_fn(|i| {
                core::array::from_fn(|j| fill::<2>(&fc.comp_ref_cdf[i][j]))
            }),
            comp_bwd_ref: core::array::from_fn(|i| {
                core::array::from_fn(|j| fill::<2>(&ic.comp_bwdref_cdf[i][j]))
            }),
            single_ref: core::array::from_fn(|i| {
                core::array::from_fn(|j| fill::<2>(&fc.single_ref_cdf[i][j]))
            }),
        }
    }
}

/// C `estimate_ref_frame_type_bits` (rd_cost.c:643-777, EXPORTED).
///
/// `ref_type` is a `MvReferenceFrame` — a single reference 1..7 or a
/// compound pair 8..; `is_compound` is the caller's flag, NOT derived
/// from the type. C sets `mbmi->ref_frame[0..2]` from the decoded pair
/// and then reads them back, so `rf` is that decoded pair.
///
/// The UNIDIR compound arm **returns early**, before the bwdref bits —
/// which is why a unidirectional pair costs strictly fewer symbols than a
/// bidirectional one.
pub fn estimate_ref_frame_type_bits(
    counts: &NeighborRefCounts,
    above: Option<NeighborMi>,
    left: Option<NeighborMi>,
    rf: [i8; 2],
    is_compound: bool,
    t: &RefFrameFacBits,
) -> u64 {
    let mut bits = 0u64;
    let mi = NeighborMi {
        ref_frame: rf,
        use_intrabc: false,
    };

    if is_compound {
        // C `has_uni_comp_refs(&mbmi->block_mi) ? UNIDIR_COMP_REFERENCE
        // : BIDIR_COMP_REFERENCE`, and that enum is **UNIDIR = 0,
        // BIDIR = 1** (definitions.h:1127-1131) — the opposite of the
        // "is it unidirectional?" boolean it reads like. A first draft
        // used the boolean and the tier-1 differential caught it on the
        // very first bidirectional pair.
        let is_unidir = mi.has_uni_comp_refs();
        let comp_ref_type = usize::from(!is_unidir);
        bits = bits.wrapping_add(
            t.comp_ref_type[comp_reference_type_context(above, left)][comp_ref_type] as u64,
        );

        if is_unidir {
            let bit = usize::from(rf[0] == BWDREF_FRAME);
            bits = bits.wrapping_add(t.uni_comp_ref[uni_comp_ref_p(counts)][0][bit] as u64);
            if bit == 0 {
                debug_assert_eq!(rf[0], LAST_FRAME);
                let bit1 = usize::from(rf[1] == LAST3_FRAME || rf[1] == GOLDEN_FRAME);
                bits = bits.wrapping_add(t.uni_comp_ref[uni_comp_ref_p1(counts)][1][bit1] as u64);
                if bit1 == 1 {
                    let bit2 = usize::from(rf[1] == GOLDEN_FRAME);
                    bits =
                        bits.wrapping_add(t.uni_comp_ref[uni_comp_ref_p2(counts)][2][bit2] as u64);
                }
            }
            // C returns HERE — no bwdref bits for a unidirectional pair.
            return bits;
        }

        let bit = usize::from(rf[0] == GOLDEN_FRAME || rf[0] == LAST3_FRAME);
        bits = bits.wrapping_add(t.comp_ref[comp_ref_p(counts)][0][bit] as u64);
        if bit == 0 {
            let bit1 = usize::from(rf[0] == LAST2_FRAME);
            bits = bits.wrapping_add(t.comp_ref[comp_ref_p1(counts)][1][bit1] as u64);
        } else {
            let bit2 = usize::from(rf[0] == GOLDEN_FRAME);
            bits = bits.wrapping_add(t.comp_ref[comp_ref_p2(counts)][2][bit2] as u64);
        }

        let bit_bwd = usize::from(rf[1] == ALTREF_FRAME);
        bits = bits.wrapping_add(t.comp_bwd_ref[comp_bwdref_p(counts)][0][bit_bwd] as u64);
        if bit_bwd == 0 {
            bits = bits.wrapping_add(
                t.comp_bwd_ref[comp_bwdref_p1(counts)][1][usize::from(rf[1] == ALTREF2_FRAME)]
                    as u64,
            );
        }
    } else {
        let rf0 = rf[0];
        let bit0 = usize::from((BWDREF_FRAME..=ALTREF_FRAME).contains(&rf0));
        bits = bits.wrapping_add(t.single_ref[single_ref_p1(counts)][0][bit0] as u64);
        if bit0 == 1 {
            let bit1 = usize::from(rf0 == ALTREF_FRAME);
            bits = bits.wrapping_add(t.single_ref[single_ref_p2(counts)][1][bit1] as u64);
            if bit1 == 0 {
                bits = bits.wrapping_add(
                    t.single_ref[single_ref_p6(counts)][5][usize::from(rf0 == ALTREF2_FRAME)]
                        as u64,
                );
            }
        } else {
            let bit2 = usize::from(rf0 == LAST3_FRAME || rf0 == GOLDEN_FRAME);
            bits = bits.wrapping_add(t.single_ref[single_ref_p3(counts)][2][bit2] as u64);
            if bit2 == 0 {
                bits = bits.wrapping_add(
                    t.single_ref[single_ref_p4(counts)][3][usize::from(rf0 != LAST_FRAME)] as u64,
                );
            } else {
                bits = bits.wrapping_add(
                    t.single_ref[single_ref_p5(counts)][4][usize::from(rf0 != LAST3_FRAME)] as u64,
                );
            }
        }
    }
    bits
}

/// C `estimate_ref_frames_num_bits` (product_coding_loop.c:8032-8063),
/// **tier 4** (`static`).
///
/// The `comp_inter` term is added ONLY when the frame header says
/// `REFERENCE_MODE_SELECT` **and** `MIN(bwidth, bheight) >= 8` — the
/// second condition is C's `is_comp_ref_allowed` inlined, and without it
/// a 4-wide block would be charged for a symbol the writer never emits.
///
/// The result is indexed by REFERENCE TYPE, not by loop position: a
/// single reference writes at `rf[0]`, a compound pair at the pair id.
/// Returns that sparse table.
#[allow(clippy::too_many_arguments)]
pub fn estimate_ref_frames_num_bits(
    ref_frame_type_arr: &[i8],
    counts: &NeighborRefCounts,
    above: Option<NeighborMi>,
    left: Option<NeighborMi>,
    reference_mode_is_select: bool,
    bwidth: u16,
    bheight: u16,
    t: &RefFrameFacBits,
    decode_ref_pair: impl Fn(i8) -> [i8; 2],
) -> Vec<(i8, u64)> {
    let (uni_term, bi_term) = if reference_mode_is_select && bwidth.min(bheight) >= 8 {
        let c = reference_mode_context(above, left);
        (t.comp_inter[c][0] as u64, t.comp_inter[c][1] as u64)
    } else {
        (0u64, 0u64)
    };

    let mut out = Vec::with_capacity(ref_frame_type_arr.len());
    for &ref_pair in ref_frame_type_arr {
        let rf = decode_ref_pair(ref_pair);
        if rf[1] == NONE_FRAME {
            let bits = estimate_ref_frame_type_bits(counts, above, left, rf, false, t) + uni_term;
            out.push((rf[0], bits));
        } else {
            let bits = estimate_ref_frame_type_bits(counts, above, left, rf, true, t) + bi_term;
            out.push((ref_pair, bits));
        }
    }
    out
}

/// C `svt_aom_have_newmv_in_inter_mode`'s sibling used by the same rate
/// path — re-exported so a caller of this module does not need a second
/// import to price a candidate.
#[inline]
pub fn is_compound_mode(mode: PredictionMode) -> bool {
    crate::inter_mv_code::is_inter_compound_mode(mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inter(rf0: i8, rf1: i8) -> NeighborMi {
        NeighborMi {
            ref_frame: [rf0, rf1],
            use_intrabc: false,
        }
    }
    fn intra() -> NeighborMi {
        NeighborMi {
            ref_frame: [INTRA_FRAME, NONE_FRAME],
            use_intrabc: false,
        }
    }

    /// TIER 4 — an INTRA neighbour contributes NOTHING to the counts, and
    /// an intrabc one increments slot 0, which no context reads.
    #[test]
    fn tier4_collect_neighbors_ref_counts_skips_intra() {
        let c = NeighborRefCounts::collect(Some(intra()), Some(intra()));
        assert_eq!(c.0, [0; TOTAL_REFS_PER_FRAME]);

        let c = NeighborRefCounts::collect(Some(inter(LAST_FRAME, NONE_FRAME)), None);
        assert_eq!(c.0[LAST_FRAME as usize], 1);

        // A compound neighbour counts BOTH references.
        let c = NeighborRefCounts::collect(Some(inter(LAST_FRAME, BWDREF_FRAME)), None);
        assert_eq!(c.0[LAST_FRAME as usize], 1);
        assert_eq!(c.0[BWDREF_FRAME as usize], 1);

        // An intrabc neighbour IS inter and lands in slot 0.
        let ibc = NeighborMi {
            ref_frame: [INTRA_FRAME, NONE_FRAME],
            use_intrabc: true,
        };
        let c = NeighborRefCounts::collect(Some(ibc), None);
        assert_eq!(c.0[0], 1);
        assert_eq!(c.0[1..], [0; 7]);
    }

    /// TIER 4 — the shared three-way vote.
    #[test]
    fn tier4_vote_is_equal_then_less_then_greater() {
        assert_eq!(vote(0, 0), 1);
        assert_eq!(vote(0, 1), 0);
        assert_eq!(vote(1, 0), 2);
    }

    /// TIER 4 — `uni_comp_ref_p1` is NOT `comp_ref_p1`: one votes LAST2
    /// against LAST3+GOLDEN, the other LAST against LAST2.
    #[test]
    fn tier4_uni_comp_ref_p1_differs_from_comp_ref_p1() {
        let mut c = NeighborRefCounts::default();
        c.0[LAST_FRAME as usize] = 2;
        c.0[LAST2_FRAME as usize] = 0;
        c.0[LAST3_FRAME as usize] = 1;
        // comp_ref_p1: LAST(2) vs LAST2(0) -> 2.
        assert_eq!(comp_ref_p1(&c), 2);
        // uni_comp_ref_p1: LAST2(0) vs LAST3+GOLDEN(1) -> 0.
        assert_eq!(uni_comp_ref_p1(&c), 0);
    }

    /// TIER 4 — an INTRA neighbour in the "one of two edges uses comp
    /// pred" arm lands in context 3, not 2.
    #[test]
    fn tier4_reference_mode_context_intra_neighbour() {
        // Above intra (single ref), left compound.
        let ctx = reference_mode_context(Some(intra()), Some(inter(LAST_FRAME, BWDREF_FRAME)));
        assert_eq!(ctx, 3);
        // Above a FORWARD single ref, left compound -> 2.
        let ctx = reference_mode_context(
            Some(inter(LAST_FRAME, NONE_FRAME)),
            Some(inter(LAST_FRAME, BWDREF_FRAME)),
        );
        assert_eq!(ctx, 2);
        // Both compound -> 4.
        assert_eq!(
            reference_mode_context(
                Some(inter(LAST_FRAME, BWDREF_FRAME)),
                Some(inter(LAST_FRAME, ALTREF_FRAME))
            ),
            4
        );
        // No edges -> 1.
        assert_eq!(reference_mode_context(None, None), 1);
        // One edge, single forward -> 0; single backward -> 1.
        assert_eq!(
            reference_mode_context(Some(inter(LAST_FRAME, NONE_FRAME)), None),
            0
        );
        assert_eq!(
            reference_mode_context(Some(inter(BWDREF_FRAME, NONE_FRAME)), None),
            1
        );
    }

    /// TIER 4 — `has_uni_comp_refs` is a same-side test, so
    /// (LAST, LAST2) is unidirectional and (LAST, BWDREF) is not.
    #[test]
    fn tier4_has_uni_comp_refs() {
        assert!(inter(LAST_FRAME, LAST2_FRAME).has_uni_comp_refs());
        assert!(inter(BWDREF_FRAME, ALTREF_FRAME).has_uni_comp_refs());
        assert!(!inter(LAST_FRAME, BWDREF_FRAME).has_uni_comp_refs());
        // A single reference is never unidirectional-compound.
        assert!(!inter(LAST_FRAME, NONE_FRAME).has_uni_comp_refs());
        // ref_frame[1] == INTRA_FRAME also means "no second ref".
        assert!(!inter(LAST_FRAME, INTRA_FRAME).has_second_ref());
    }

    fn ones() -> RefFrameFacBits {
        RefFrameFacBits {
            comp_inter: [[1i32; 2]; COMP_INTER_CONTEXTS],
            comp_ref_type: [[1i32; 2]; COMP_REF_TYPE_CONTEXTS],
            uni_comp_ref: [[[1i32; 2]; 3]; REF_CONTEXTS],
            comp_ref: [[[1i32; 2]; 3]; REF_CONTEXTS],
            comp_bwd_ref: [[[1i32; 2]; 2]; REF_CONTEXTS],
            single_ref: [[[1i32; 2]; 6]; REF_CONTEXTS],
        }
    }

    /// TIER 4 — a unidirectional compound pair pays strictly FEWER
    /// symbols than a bidirectional one, because C returns early.
    #[test]
    fn tier4_unidir_compound_returns_before_the_bwdref_bits() {
        let t = ones();
        let c = NeighborRefCounts::default();
        // (LAST, LAST2): type + uni_comp_ref[0] + uni_comp_ref[1] = 3.
        let uni = estimate_ref_frame_type_bits(&c, None, None, [LAST_FRAME, LAST2_FRAME], true, &t);
        assert_eq!(uni, 3);
        // (LAST, BWDREF): type + comp_ref[0] + comp_ref[1] +
        // comp_bwd_ref[0] + comp_bwd_ref[1] = 5.
        let bi = estimate_ref_frame_type_bits(&c, None, None, [LAST_FRAME, BWDREF_FRAME], true, &t);
        assert_eq!(bi, 5);
        // (BWDREF, ALTREF) is unidirectional too, and takes the
        // `bit == 1` arm, so it stops after ONE uni_comp_ref symbol.
        let uni_bwd =
            estimate_ref_frame_type_bits(&c, None, None, [BWDREF_FRAME, ALTREF_FRAME], true, &t);
        assert_eq!(uni_bwd, 2);
    }

    /// TIER 4 — every single reference costs two or three symbols, and
    /// exactly which ones depends on the reference.
    #[test]
    fn tier4_single_ref_symbol_counts() {
        let t = ones();
        let c = NeighborRefCounts::default();
        let n = |rf: i8| estimate_ref_frame_type_bits(&c, None, None, [rf, NONE_FRAME], false, &t);
        // LAST: p1 + p3 + p4 = 3.
        assert_eq!(n(LAST_FRAME), 3);
        assert_eq!(n(LAST2_FRAME), 3);
        assert_eq!(n(LAST3_FRAME), 3);
        assert_eq!(n(GOLDEN_FRAME), 3);
        // BWDREF: p1 + p2 + p6 = 3.
        assert_eq!(n(BWDREF_FRAME), 3);
        assert_eq!(n(ALTREF2_FRAME), 3);
        // ALTREF: p1 + p2 = 2 (bit1 == 1 skips p6).
        assert_eq!(n(ALTREF_FRAME), 2);
    }

    /// TIER 4 — the comp_inter term is gated on BOTH the frame-header
    /// mode and `MIN(bwidth, bheight) >= 8`.
    #[test]
    fn tier4_estimate_ref_frames_num_bits_comp_inter_gate() {
        let t = ones();
        let c = NeighborRefCounts::default();
        let decode = |p: i8| {
            if p < 8 {
                [p, NONE_FRAME]
            } else {
                [LAST_FRAME, BWDREF_FRAME]
            }
        };

        let with =
            estimate_ref_frames_num_bits(&[LAST_FRAME], &c, None, None, true, 16, 16, &t, decode);
        let without =
            estimate_ref_frames_num_bits(&[LAST_FRAME], &c, None, None, false, 16, 16, &t, decode);
        assert_eq!(with[0].1, without[0].1 + 1);

        // A 4-wide block loses the term even under REFERENCE_MODE_SELECT.
        let narrow =
            estimate_ref_frames_num_bits(&[LAST_FRAME], &c, None, None, true, 4, 16, &t, decode);
        assert_eq!(narrow[0].1, without[0].1);

        // The result is keyed by reference TYPE: a single ref by rf[0],
        // a compound pair by the pair id.
        let mixed = estimate_ref_frames_num_bits(
            &[LAST_FRAME, 8],
            &c,
            None,
            None,
            false,
            16,
            16,
            &t,
            decode,
        );
        assert_eq!(mixed[0].0, LAST_FRAME);
        assert_eq!(mixed[1].0, 8);
    }
}

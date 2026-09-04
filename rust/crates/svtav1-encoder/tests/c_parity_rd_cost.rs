//! Differential parity: the MD cost layer
//! (`svtav1-encoder/src/port_rd_cost/`) vs the REAL exported C.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4). The oracles are the
//! EXPORTED `svt_aom_get_switchable_rate` (rd_cost.c:849),
//! `svt_aom_inter_fast_cost` (:1005), `svt_aom_intra_fast_cost` (:526),
//! `svt_aom_get_intra_uv_fast_rate` (:476), `svt_aom_full_cost` (:1349) and
//! `svt_aom_full_cost_pd0` (:1330), driven through `svtav1-cref`'s `rd_cost`
//! shim over randomized candidates, contexts and rate tables.
//!
//! Two C `static`s this lane also ports are reached THROUGH those oracles
//! rather than re-transcribed into a second shim:
//! `av1_inter_fast_cost_light` (:870) via `approx_inter_rate`, and
//! `get_compound_mode_rate` (:783) unconditionally on every compound
//! candidate.
//!
//! What these tests deliberately do NOT reach, said here rather than implied:
//!
//! * the `use_palette == 1` arm of `svt_aom_intra_fast_cost` — the port takes
//!   C's assembled `palette_mode_cost` as an input (see the module doc), so
//!   driving it here would compare C against a value C produced. Every cell
//!   below runs with `palette_info == NULL`, which is what C sees for every
//!   non-palette candidate and which still exercises the `palette_ymode` and
//!   `palette_uv_mode` terms;
//! * the tx-size terms of `svt_aom_full_cost` — C recomputes them from
//!   `svt_aom_get_tx_size_bits` rather than taking them as arguments, so the
//!   cells run at `tx_mode != TX_MODE_SELECT` where BOTH are zero on both
//!   sides. `crate::vartx`'s own differential covers that walk.

use svtav1_cref::rd_cost as cref;
use svtav1_encoder::entropy::context::{
    BLOCK_SIZE_GROUPS, BLOCK_SIZES_ALL, DRL_MODE_CONTEXTS, GLOBALMV_MODE_CONTEXTS,
    INTER_MODE_CONTEXTS, INTRA_INTER_CONTEXTS, INTRA_MODES, KF_MODE_CONTEXTS, NEWMV_MODE_CONTEXTS,
    REFMV_MODE_CONTEXTS, SKIP_MODE_CONTEXTS, SWITCHABLE_FILTERS, UV_INTRA_MODES,
};
use svtav1_encoder::port_entropy_inter::interp::SWITCHABLE_FILTER_CONTEXTS;
use svtav1_encoder::port_entropy_inter::modes::{MOTION_MODES, MotionMode, TransformationType};
use svtav1_encoder::port_entropy_inter::{NeighborMi, Neighbors};
use svtav1_encoder::port_md::pme::{MV_MAX, MV_VALS, MvCostTable};
use svtav1_encoder::port_rd_cost::full_cost as rfull;
use svtav1_encoder::port_rd_cost::inter_cost as rinter;
use svtav1_encoder::port_rd_cost::intra_cost as rintra;
use svtav1_types::block::BlockSize;
use svtav1_types::motion::{CandidateMv, MAX_REF_MV_STACK_SIZE, Mv};
use svtav1_types::prediction::{CompoundType, PredictionMode};

// ---------------------------------------------------------------------------
// RNG + neighbour plumbing
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn rate(&mut self) -> i32 {
        // Fac-bits are non-negative and a few thousand at most.
        (self.next() % 6000) as i32
    }
    fn flag(&mut self) -> bool {
        self.next() & 1 == 1
    }
}

/// One neighbour, in both representations at once.
fn neighbor(r: &mut Rng) -> ([i32; cref::NB_FIELDS], NeighborMi, bool) {
    let valid = !r.next().is_multiple_of(4);
    let mi = NeighborMi {
        mode: (r.below(25)) as u8,
        ref_frame: [(r.below(9) as i32 - 1) as i8, (r.below(9) as i32 - 1) as i8],
        interp_filters: (r.below(3) as u32) | ((r.below(3) as u32) << 16),
        use_intrabc: r.flag(),
        skip_mode: r.flag(),
        comp_group_idx: (r.next() & 1) as u8,
        compound_idx: (r.next() & 1) as u8,
        bsize: r.below(BLOCK_SIZES_ALL as u64) as u8,
    };
    let f = [
        i32::from(valid),
        mi.mode as i32,
        mi.ref_frame[0] as i32,
        mi.ref_frame[1] as i32,
        mi.interp_filters as i32,
        i32::from(mi.use_intrabc),
        i32::from(mi.skip_mode),
        mi.comp_group_idx as i32,
        mi.compound_idx as i32,
        mi.bsize as i32,
    ];
    (f, mi, valid)
}

/// The neighbour pair, with AVAILABILITY TIED TO POINTER VALIDITY.
///
/// C's `MacroBlockD` sets `above_mbmi = mi[-mi_stride]` when `up_available`
/// and NULL otherwise (`svt_aom_md_init_xd`), so the two are one fact in the
/// encoder. They are separate knobs in `entropy_inter`'s harness because
/// different context functions read different ones, and that harness needs
/// the distinction; here they must AGREE, because
/// `svt_aom_get_pred_context_switchable_interp` reads the mi GRID (always
/// populated in a shim) gated on `up_available`, while the port reads the
/// pointer. Feeding "available but null" would compare the port against a
/// state the encoder cannot produce and call the disagreement a bug.
fn neighbors(
    r: &mut Rng,
) -> (
    [i32; cref::NB_FIELDS],
    [i32; cref::NB_FIELDS],
    Neighbors,
    bool,
    bool,
) {
    let (mut af, ami, avalid) = neighbor(r);
    let (mut lf, lmi, lvalid) = neighbor(r);
    let up_avail = avalid;
    let left_avail = lvalid;
    af[0] = i32::from(avalid);
    lf[0] = i32::from(lvalid);
    let nb = Neighbors {
        above: if avalid { Some(ami) } else { None },
        left: if lvalid { Some(lmi) } else { None },
        up_available: up_avail,
        left_available: left_avail,
    };
    (af, lf, nb, up_avail, left_avail)
}

// ---------------------------------------------------------------------------
// Rate-table packing — the ORDER here is the shim's `rd_scatter_*` contract
// ---------------------------------------------------------------------------

/// Build a random [`rinter::InterFacBits`] and its flat companion, in the
/// order `rd_scatter_inter` reads.
fn inter_tables(r: &mut Rng) -> (rinter::InterFacBits, Vec<i32>) {
    let mut t = rinter::InterFacBits::zeroed();
    let mut flat = Vec::new();
    macro_rules! fill2 {
        ($f:expr) => {
            for row in $f.iter_mut() {
                for v in row.iter_mut() {
                    *v = r.rate();
                    flat.push(*v);
                }
            }
        };
    }
    fill2!(t.skip_mode);
    fill2!(t.intra_inter);
    fill2!(t.new_mv_mode);
    fill2!(t.zero_mv_mode);
    fill2!(t.ref_mv_mode);
    fill2!(t.drl_mode);
    fill2!(t.inter_compound_mode);
    fill2!(t.switchable_interp);
    fill2!(t.motion_mode);
    fill2!(t.motion_mode1);
    fill2!(t.inter_intra);
    fill2!(t.inter_intra_mode);
    fill2!(t.wedge_inter_intra);
    fill2!(t.wedge_idx);
    fill2!(t.comp_group_idx);
    fill2!(t.comp_idx);
    fill2!(t.compound_type);
    (t, flat)
}

/// Same for the intra tables, in `rd_scatter_intra` order.
fn intra_tables(r: &mut Rng) -> (rintra::IntraFacBits, Vec<i32>) {
    let mut t = rintra::IntraFacBits::zeroed();
    let mut flat = Vec::new();
    for a in t.y_mode.iter_mut() {
        for b in a.iter_mut() {
            for v in b.iter_mut() {
                *v = r.rate();
                flat.push(*v);
            }
        }
    }
    macro_rules! fill2 {
        ($f:expr) => {
            for row in $f.iter_mut() {
                for v in row.iter_mut() {
                    *v = r.rate();
                    flat.push(*v);
                }
            }
        };
    }
    fill2!(t.mb_mode);
    for a in t.intra_uv_mode.iter_mut() {
        for b in a.iter_mut() {
            for v in b.iter_mut() {
                *v = r.rate();
                flat.push(*v);
            }
        }
    }
    fill2!(t.angle_delta);
    for a in t.cfl_alpha.iter_mut() {
        for b in a.iter_mut() {
            for v in b.iter_mut() {
                *v = r.rate();
                flat.push(*v);
            }
        }
    }
    fill2!(t.filter_intra);
    for v in t.filter_intra_mode.iter_mut() {
        *v = r.rate();
        flat.push(*v);
    }
    for a in t.palette_ymode.iter_mut() {
        for b in a.iter_mut() {
            for v in b.iter_mut() {
                *v = r.rate();
                flat.push(*v);
            }
        }
    }
    fill2!(t.palette_uv_mode);
    fill2!(t.intra_inter);
    fill2!(t.skip_mode);
    for v in t.intrabc.iter_mut() {
        *v = r.rate();
        flat.push(*v);
    }
    (t, flat)
}

/// A random MV cost table plus the flat `[2][MV_VALS]` C wants. Only the
/// window a legal MV difference can reach is populated; the rest is a
/// constant both sides read identically.
fn mv_tables(r: &mut Rng) -> (MvCostTable, [i32; 4], Vec<i32>) {
    let joint = [r.rate(), r.rate(), r.rate(), r.rate()];
    let mut flat = vec![0i32; 2 * MV_VALS];
    let mut comp: [Vec<i32>; 2] = [vec![0i32; MV_VALS], vec![0i32; MV_VALS]];
    for c in 0..2usize {
        for d in -512i32..=512 {
            let idx = (MV_MAX + d) as usize;
            let v = r.rate();
            comp[c][idx] = v;
            flat[c * MV_VALS + idx] = v;
        }
    }
    let [comp0, comp1] = comp;
    (
        MvCostTable {
            joint_cost: joint,
            comp_cost: [
                svtav1_encoder::intrabc::MvComponentCost::from_table(comp0),
                svtav1_encoder::intrabc::MvComponentCost::from_table(comp1),
            ],
        },
        joint,
        flat,
    )
}

// ---------------------------------------------------------------------------
// 0. layout agreement
// ---------------------------------------------------------------------------

#[test]
fn shim_layout_matches_the_rust_side() {
    let (nb, ifc, intra, full, inter_len, intra_len, mv_vals) = cref::layout();
    assert_eq!(nb, cref::NB_FIELDS);
    assert_eq!(ifc, cref::IFC_FIELDS);
    assert_eq!(intra, cref::INTRA_FIELDS);
    assert_eq!(full, cref::FULL_FIELDS);
    assert_eq!(mv_vals, cref::MV_VALS);
    assert_eq!(mv_vals, MV_VALS);

    let mut r = Rng(1);
    let (_, flat) = inter_tables(&mut r);
    assert_eq!(
        flat.len(),
        inter_len,
        "the inter table pack and the shim's scatter disagree on length"
    );
    let (_, flat) = intra_tables(&mut r);
    assert_eq!(
        flat.len(),
        intra_len,
        "the intra table pack and the shim's scatter disagree on length"
    );

    // The dimensions the pack is built from, pinned so a table resize cannot
    // silently shift every following field.
    assert_eq!(SKIP_MODE_CONTEXTS, 3);
    assert_eq!(INTRA_INTER_CONTEXTS, 4);
    assert_eq!(NEWMV_MODE_CONTEXTS, 6);
    assert_eq!(GLOBALMV_MODE_CONTEXTS, 2);
    assert_eq!(REFMV_MODE_CONTEXTS, 6);
    assert_eq!(DRL_MODE_CONTEXTS, 3);
    assert_eq!(INTER_MODE_CONTEXTS, 8);
    assert_eq!(SWITCHABLE_FILTER_CONTEXTS, 16);
    assert_eq!(SWITCHABLE_FILTERS, 3);
    assert_eq!(MOTION_MODES, 3);
    assert_eq!(BLOCK_SIZE_GROUPS, 4);
    assert_eq!(BLOCK_SIZES_ALL, 22);
    assert_eq!(KF_MODE_CONTEXTS, 5);
    assert_eq!(INTRA_MODES, 13);
    assert_eq!(UV_INTRA_MODES, 14);
}

// ---------------------------------------------------------------------------
// 1. svt_aom_get_switchable_rate
// ---------------------------------------------------------------------------

#[test]
fn switchable_rate_matches_c() {
    let mut r = Rng(0x51c7_ab1e);
    for _ in 0..3000 {
        let (t, _flat) = inter_tables(&mut r);
        let mut tbl = Vec::with_capacity(SWITCHABLE_FILTER_CONTEXTS * SWITCHABLE_FILTERS);
        for row in t.switchable_interp.iter() {
            tbl.extend_from_slice(row);
        }
        let (af, lf, nb, up, left) = neighbors(&mut r);
        // SWITCHABLE is 3; anything else must cost zero.
        let interp_filter = r.below(5) as i32;
        // rf[0] is 1..=7 (LAST..ALTREF); rf[1] is -1 (NONE) or 1..=7.
        let rf = [
            (r.below(7) as i32) + 1,
            if r.flag() {
                -1
            } else {
                (r.below(7) as i32) + 1
            },
        ];
        let interp_filters = (r.below(3) as u32) | ((r.below(3) as u32) << 16);
        let dual = r.flag();

        let c = cref::switchable_rate(
            interp_filter,
            rf,
            interp_filters,
            &af,
            &lf,
            up,
            left,
            dual,
            &tbl,
        );
        let p = rinter::get_switchable_rate(
            interp_filter as u8,
            [rf[0] as i8, rf[1] as i8],
            interp_filters,
            &nb,
            dual,
            &t,
        );
        assert_eq!(c, p, "interp_filter={interp_filter} rf={rf:?} dual={dual}");
    }
}

// ---------------------------------------------------------------------------
// 2/3. svt_aom_inter_fast_cost (full + light) and get_compound_mode_rate
// ---------------------------------------------------------------------------

/// C `NEARESTMV..NEWMV` in enum order.
const SINGLE_MODES: [PredictionMode; 4] = [
    PredictionMode::NearestMv,
    PredictionMode::NearMv,
    PredictionMode::GlobalMv,
    PredictionMode::NewMv,
];

/// C `NEAREST_NEARESTMV..NEW_NEWMV` in enum order.
const COMPOUND_MODES: [PredictionMode; 8] = [
    PredictionMode::NearestNearestMv,
    PredictionMode::NearNearMv,
    PredictionMode::NearestNewMv,
    PredictionMode::NewNearestMv,
    PredictionMode::NearNewMv,
    PredictionMode::NewNearMv,
    PredictionMode::GlobalGlobalMv,
    PredictionMode::NewNewMv,
];

struct InterCell {
    fields: [i32; cref::IFC_FIELDS],
    cand: rinter::InterCandidate,
    bsize: BlockSize,
    inter_mode_ctx: i16,
    ref_mv_count: u8,
    approx: u8,
    ifs_at_mds0: bool,
    skip_mode_ctx: usize,
    is_inter_ctx: usize,
    overlappable: u32,
    frame_flags: [i32; 13],
}

/// Build a legal candidate. `compound` picks between the single-reference and
/// compound halves of the mode space; both halves are exercised.
fn inter_cell(r: &mut Rng, compound: bool, approx: u8) -> InterCell {
    let bsize = BlockSize::from_u8(r.below(BLOCK_SIZES_ALL as u64) as u8).unwrap();
    let mode = if compound {
        // NEAREST_NEARESTMV..NEW_NEWMV
        COMPOUND_MODES[r.below(8) as usize]
    } else {
        // NEARESTMV..NEWMV
        SINGLE_MODES[r.below(4) as usize]
    };
    let rf: [i8; 2] = if compound {
        // One forward (1..4) and one backward (5..7) reference: a BIDIR pair,
        // whose `av1_ref_frame_type` is inside MODE_CTX_REF_FRAMES.
        [1 + r.below(4) as i8, 5 + r.below(3) as i8]
    } else {
        [1 + r.below(7) as i8, -1]
    };

    // A mode context whose three extracted fields are all inside their
    // tables: newmv 0..5, globalmv 0..1, refmv 0..5. Real contexts never
    // exceed those; a wider draw would index past `new_mv_mode_fac_bits`
    // in C too.
    let inter_mode_ctx =
        (r.below(6) as i16) | ((r.below(2) as i16) << 3) | ((r.below(6) as i16) << 4);

    let mv = |r: &mut Rng| Mv {
        x: (r.below(1024) as i32 - 512) as i16,
        y: (r.below(1024) as i32 - 512) as i16,
    };
    let comp_group_idx = if compound && r.flag() { 1u8 } else { 0 };
    let interinter_comp_type = if comp_group_idx == 1 {
        if r.flag() {
            CompoundType::Wedge
        } else {
            CompoundType::DiffWtd
        }
    } else {
        CompoundType::Average
    };

    let cand = rinter::InterCandidate {
        mode,
        ref_frame: rf,
        mv: [mv(r), mv(r)],
        pred_mv: [mv(r), mv(r)],
        drl_index: r.below(3) as u8,
        interp_filters: (r.below(3) as u32) | ((r.below(3) as u32) << 16),
        motion_mode: match r.below(3) {
            0 => MotionMode::SimpleTranslation,
            1 => MotionMode::ObmcCausal,
            _ => MotionMode::WarpedCausal,
        },
        num_proj_ref: r.below(3) as u16,
        is_interintra_used: r.flag(),
        interintra_mode: r.below(4) as u8,
        use_wedge_interintra: r.flag(),
        interintra_wedge_index: r.below(16) as u8,
        comp_group_idx,
        compound_idx: (r.next() & 1) as u8,
        interinter_comp_type,
        interinter_wedge_index: r.below(16) as u8,
        skip_mode_allowed: r.flag(),
    };

    let skip_mode_ctx = r.below(SKIP_MODE_CONTEXTS as u64) as usize;
    let is_inter_ctx = r.below(INTRA_INTER_CONTEXTS as u64) as usize;
    let ref_mv_count = r.below(5) as u8;
    let ifs_at_mds0 = r.flag();
    let overlappable = r.below(3) as u32;
    // 31..43 of the field list, in order.
    let frame_flags = [
        r.below(5) as i32,     // interpolation_filter
        i32::from(r.flag()),   // skip_mode_flag
        i32::from(r.flag()),   // is_motion_mode_switchable
        i32::from(r.flag()),   // force_integer_mv
        i32::from(r.flag()),   // allow_warped_motion
        i32::from(r.flag()),   // enable_dual_filter
        i32::from(r.flag()),   // enable_masked_compound
        i32::from(r.flag()),   // enable_jnt_comp
        i32::from(r.flag()),   // enable_interintra_compound
        i32::from(r.flag()),   // enable_order_hint
        3 + r.below(5) as i32, // order_hint_bits
        r.below(64) as i32,    // cur_order_hint
        i32::from(r.flag()),   // allow_screen_content_tools
    ];

    let mut fields = [0i32; cref::IFC_FIELDS];
    fields[0] = bsize.as_index() as i32;
    fields[1] = mode as i32;
    fields[2] = rf[0] as i32;
    fields[3] = rf[1] as i32;
    fields[4] = cand.mv[0].x as i32;
    fields[5] = cand.mv[0].y as i32;
    fields[6] = cand.mv[1].x as i32;
    fields[7] = cand.mv[1].y as i32;
    fields[8] = cand.pred_mv[0].x as i32;
    fields[9] = cand.pred_mv[0].y as i32;
    fields[10] = cand.pred_mv[1].x as i32;
    fields[11] = cand.pred_mv[1].y as i32;
    fields[12] = cand.drl_index as i32;
    fields[13] = cand.interp_filters as i32;
    fields[14] = cand.motion_mode as i32;
    fields[15] = cand.num_proj_ref as i32;
    fields[16] = i32::from(cand.is_interintra_used);
    fields[17] = cand.interintra_mode as i32;
    fields[18] = i32::from(cand.use_wedge_interintra);
    fields[19] = cand.interintra_wedge_index as i32;
    fields[20] = cand.comp_group_idx as i32;
    fields[21] = cand.compound_idx as i32;
    fields[22] = cand.interinter_comp_type as i32;
    fields[23] = cand.interinter_wedge_index as i32;
    fields[24] = i32::from(cand.skip_mode_allowed);
    fields[25] = skip_mode_ctx as i32;
    fields[26] = is_inter_ctx as i32;
    fields[27] = inter_mode_ctx as i32;
    fields[28] = ref_mv_count as i32;
    fields[29] = approx as i32;
    fields[30] = i32::from(ifs_at_mds0);
    fields[31..44].copy_from_slice(&frame_flags);
    fields[46] = overlappable as i32;

    InterCell {
        fields,
        cand,
        bsize,
        inter_mode_ctx,
        ref_mv_count,
        approx,
        ifs_at_mds0,
        skip_mode_ctx,
        is_inter_ctx,
        overlappable,
        frame_flags,
    }
}

fn run_inter_cells(seed: u64, compound: bool, approx: u8, cells: usize) {
    let mut r = Rng(seed);
    for _ in 0..cells {
        let (t, flat) = inter_tables(&mut r);
        let (mv_tbl, joint, mv_flat) = mv_tables(&mut r);
        let mut cell = inter_cell(&mut r, compound, approx);
        let (af, lf, nb, up, left) = neighbors(&mut r);
        cell.fields[44] = i32::from(up);
        cell.fields[45] = i32::from(left);

        let ref_order_hint: [i32; 7] = core::array::from_fn(|_| r.below(64) as i32);
        let gm_wmtype: [i32; 8] = core::array::from_fn(|_| r.below(4) as i32);
        let stack_weights: [i32; 8] = core::array::from_fn(|_| r.below(1400) as i32);
        let ref_frames_num_bits = r.below(20_000);
        let lambda = 1 + r.below(60_000);
        let dist = r.below(1_000_000);

        let (c_cost, c_luma, c_chroma) = cref::inter_fast_cost(
            &cell.fields,
            &af,
            &lf,
            &flat,
            &joint,
            &mv_flat,
            &ref_order_hint,
            &gm_wmtype,
            &stack_weights,
            ref_frames_num_bits,
            lambda,
            dist,
        );

        let gm: [TransformationType; 8] = core::array::from_fn(|i| match gm_wmtype[i] {
            0 => TransformationType::Identity,
            1 => TransformationType::Translation,
            2 => TransformationType::RotZoom,
            _ => TransformationType::Affine,
        });
        let ff = cell.frame_flags;
        let frame = rinter::InterFrame {
            allow_screen_content_tools: ff[12] != 0,
            skip_mode_flag: ff[1] != 0,
            interpolation_filter: ff[0] as u8,
            is_motion_mode_switchable: ff[2] != 0,
            force_integer_mv: ff[3] != 0,
            allow_warped_motion: ff[4] != 0,
            enable_dual_filter: ff[5] != 0,
            enable_masked_compound: ff[6] != 0,
            enable_jnt_comp: ff[7] != 0,
            enable_interintra_compound: ff[8] != 0,
            enable_order_hint: ff[9] != 0,
            order_hint_bits: ff[10] as u32,
            cur_order_hint: ff[11],
            ref_order_hint: &ref_order_hint,
            gm_wmtype: &gm,
        };
        let stack: Vec<CandidateMv> = (0..MAX_REF_MV_STACK_SIZE)
            .map(|i| CandidateMv {
                this_mv: Mv { x: 0, y: 0 },
                comp_mv: Mv { x: 0, y: 0 },
                weight: stack_weights[i],
            })
            .collect();
        let block = rinter::InterBlock {
            bsize: cell.bsize,
            skip_mode_ctx: cell.skip_mode_ctx,
            is_inter_ctx: cell.is_inter_ctx,
            inter_mode_ctx: cell.inter_mode_ctx,
            ref_mv_count: cell.ref_mv_count,
            ref_mv_stack: &stack,
            ref_frames_num_bits,
            neighbors: &nb,
            overlappable_neighbors: cell.overlappable,
            approx_inter_rate: cell.approx,
            ifs_at_mds0: cell.ifs_at_mds0,
        };
        let got =
            rinter::inter_fast_cost(&frame, &block, &cell.cand, lambda, dist, Some(&mv_tbl), &t);

        assert_eq!(
            (got.cost, got.rate.luma, got.rate.chroma),
            (c_cost, c_luma, c_chroma),
            "compound={compound} approx={approx} mode={:?} bsize={:?} rf={:?}",
            cell.cand.mode,
            cell.bsize,
            cell.cand.ref_frame,
        );
    }
}

#[test]
fn inter_fast_cost_singleref_matches_c() {
    run_inter_cells(0x1234_5678, false, 0, 1200);
}

#[test]
fn inter_fast_cost_compound_matches_c() {
    run_inter_cells(0x9abc_def0, true, 0, 1200);
}

#[test]
fn inter_fast_cost_light_matches_c() {
    // approx_inter_rate 1 and 2 differ: 2 also drops the reference bits.
    run_inter_cells(0x0f1e_2d3c, false, 1, 600);
    run_inter_cells(0x0f1e_2d3d, true, 1, 600);
    run_inter_cells(0x4b5a_6978, false, 2, 600);
    run_inter_cells(0x4b5a_6979, true, 2, 600);
}

// ---------------------------------------------------------------------------
// 4/5. svt_aom_get_intra_uv_fast_rate and svt_aom_intra_fast_cost
// ---------------------------------------------------------------------------

struct IntraCell {
    fields: [i32; cref::INTRA_FIELDS],
    block: rintra::IntraBlock,
    cand: rintra::IntraCandidate,
}

fn intra_cell(r: &mut Rng, use_accurate_cfl: bool, hdr_allow_intrabc: bool) -> IntraCell {
    let bsize = BlockSize::from_u8(r.below(BLOCK_SIZES_ALL as u64) as u8).unwrap();
    let bi = bsize.as_index();
    let bwidth = u16::from(svtav1_types::tables::block::BLOCK_SIZE_WIDE[bi]);
    let bheight = u16::from(svtav1_types::tables::block::BLOCK_SIZE_HIGH[bi]);
    let is_key = r.flag() || hdr_allow_intrabc;
    // C `svt_aom_allow_intrabc` (entropy_coding.c:4401) is a CONJUNCTION of
    // THREE things: `slice_type == I_SLICE && allow_screen_content_tools &&
    // frm_hdr->allow_intrabc`. The port takes the resolved boolean, so the
    // test has to resolve it the same way — a first draft dropped the
    // screen-content term and the differential caught it as a rate gap of
    // exactly the terms C then skipped.
    let scr = hdr_allow_intrabc || r.flag();
    let allow_intrabc = hdr_allow_intrabc && is_key && scr;
    let use_intrabc = allow_intrabc && r.flag();

    let block = rintra::IntraBlock {
        bsize,
        bwidth,
        bheight,
        is_key_slice: is_key,
        allow_intrabc,
        allow_screen_content_tools: scr,
        skip_mode_flag: r.flag(),
        filter_intra_level: r.below(2) as u8,
        has_uv: r.flag(),
        intra_luma_top_ctx: r.below(KF_MODE_CONTEXTS as u64) as usize,
        intra_luma_left_ctx: r.below(KF_MODE_CONTEXTS as u64) as usize,
        is_inter_ctx: r.below(INTRA_INTER_CONTEXTS as u64) as usize,
        skip_mode_ctx: r.below(SKIP_MODE_CONTEXTS as u64) as usize,
        palette_bsize_ctx: 0,
        palette_mode_ctx: 0,
        mi_row: 0,
        mi_col: 0,
    };
    let cand = rintra::IntraCandidate {
        mode: r.below(INTRA_MODES as u64) as u8,
        uv_mode: r.below(UV_INTRA_MODES as u64) as u8,
        angle_delta_y: r.below(7) as i32 - 3,
        angle_delta_uv: r.below(7) as i32 - 3,
        cfl_alpha_signs: r.below(8) as u8,
        cfl_alpha_idx: r.below(256) as u8,
        filter_intra_mode: r.below(6) as u8,
        // `palette_info == NULL` for every cell — see the module doc.
        palette: None,
        palette_mode_cost: 0,
        use_intrabc,
        mv: Mv {
            x: (r.below(512) as i32 - 256) as i16,
            y: (r.below(512) as i32 - 256) as i16,
        },
        pred_mv: Mv {
            x: (r.below(512) as i32 - 256) as i16,
            y: (r.below(512) as i32 - 256) as i16,
        },
    };

    // The palette contexts are inputs on both sides: with `palette_info`
    // NULL C reads `palette_ymode_fac_bits[bsize_ctx][mode_ctx][0]` and the
    // shim's ctx values must equal the port's. C derives them from the block
    // and the neighbours; here both sides are pinned to 0 by construction
    // (the shim leaves the mi grid's palette sizes zero, which is
    // `get_palette_mode_ctx == 0`), and `svt_aom_get_palette_bsize_ctx` is
    // driven by giving the port the value C computes for this bsize.
    let mut block = block;
    // `svt_aom_get_palette_bsize_ctx` = num_pels_log2 - num_pels_log2[8x8],
    // defined only where palette is allowed (bsize >= BLOCK_8X8, i.e. >= 64
    // pels). Below that the port never reads the context, so 0 stands in.
    let pels = usize::from(bwidth) * usize::from(bheight);
    block.palette_bsize_ctx = pels.trailing_zeros().saturating_sub(6) as usize;
    block.palette_mode_ctx = 0;

    let mut fields = [0i32; cref::INTRA_FIELDS];
    fields[0] = bi as i32;
    fields[1] = cand.mode as i32;
    fields[2] = cand.uv_mode as i32;
    fields[3] = cand.angle_delta_y;
    fields[4] = cand.angle_delta_uv;
    fields[5] = cand.cfl_alpha_signs as i32;
    fields[6] = cand.cfl_alpha_idx as i32;
    fields[7] = cand.filter_intra_mode as i32;
    fields[8] = 0; // palette_size[0] — palette_info is NULL
    fields[9] = 0; // palette_size[1]
    fields[10] = i32::from(cand.use_intrabc);
    fields[11] = i32::from(block.is_key_slice);
    fields[12] = i32::from(hdr_allow_intrabc);
    fields[13] = i32::from(block.allow_screen_content_tools);
    fields[14] = i32::from(block.skip_mode_flag);
    fields[15] = block.filter_intra_level as i32;
    fields[16] = i32::from(block.has_uv);
    fields[17] = block.intra_luma_top_ctx as i32;
    fields[18] = block.intra_luma_left_ctx as i32;
    fields[19] = block.is_inter_ctx as i32;
    fields[20] = block.skip_mode_ctx as i32;
    fields[21] = 0; // blk_org_x
    fields[22] = 0; // blk_org_y
    fields[25] = i32::from(use_accurate_cfl);
    fields[26] = cand.mv.x as i32;
    fields[27] = cand.mv.y as i32;
    fields[28] = cand.pred_mv.x as i32;
    fields[29] = cand.pred_mv.y as i32;

    IntraCell {
        fields,
        block,
        cand,
    }
}

#[test]
fn intra_uv_fast_rate_matches_c() {
    let mut r = Rng(0xfeed_face);
    for _ in 0..3000 {
        let (t, flat) = intra_tables(&mut r);
        let (dv_tbl, dv_joint, dv_flat) = mv_tables(&mut r);
        let _ = &dv_tbl;
        let accurate = r.flag();
        let mut cell = intra_cell(&mut r, accurate, false);
        cell.block.has_uv = true;
        cell.fields[16] = 1;
        let (af, lf, _nb, up, left) = neighbors(&mut r);
        cell.fields[23] = i32::from(up);
        cell.fields[24] = i32::from(left);

        let c = cref::intra_uv_fast_rate(&cell.fields, &af, &lf, &flat, &dv_joint, &dv_flat);
        let p = rintra::get_intra_uv_fast_rate(&cell.block, &cell.cand, accurate, &t);
        assert_eq!(
            c, p,
            "bsize={:?} mode={} uv_mode={} accurate={accurate}",
            cell.block.bsize, cell.cand.mode, cell.cand.uv_mode
        );
    }
}

fn run_intra_cells(seed: u64, allow_intrabc: bool, cells: usize) {
    let mut r = Rng(seed);
    for _ in 0..cells {
        let (t, flat) = intra_tables(&mut r);
        let (dv_tbl, dv_joint, dv_flat) = mv_tables(&mut r);
        let mut cell = intra_cell(&mut r, false, allow_intrabc);
        let (af, lf, _nb, up, left) = neighbors(&mut r);
        cell.fields[23] = i32::from(up);
        cell.fields[24] = i32::from(left);
        let lambda = 1 + r.below(60_000);
        let dist = r.below(1_000_000);

        let (c_cost, c_luma, c_chroma) = cref::intra_fast_cost(
            &cell.fields,
            &af,
            &lf,
            &flat,
            &dv_joint,
            &dv_flat,
            lambda,
            dist,
        );
        let got = rintra::intra_fast_cost(&cell.block, &cell.cand, lambda, dist, Some(&dv_tbl), &t);
        assert_eq!(
            (got.cost, got.fast_luma_rate, got.fast_chroma_rate),
            (c_cost, u64::from(c_luma), u64::from(c_chroma)),
            "bsize={:?} mode={} uv={} ibc={} key={}",
            cell.block.bsize,
            cell.cand.mode,
            cell.cand.uv_mode,
            cell.cand.use_intrabc,
            cell.block.is_key_slice,
        );
    }
}

#[test]
fn intra_fast_cost_matches_c() {
    run_intra_cells(0xa1b2_c3d4, false, 2000);
}

#[test]
fn intra_fast_cost_intrabc_matches_c() {
    run_intra_cells(0x5566_7788, true, 2000);
}

// ---------------------------------------------------------------------------
// 6/7. svt_aom_full_cost and svt_aom_full_cost_pd0
// ---------------------------------------------------------------------------

#[test]
fn full_cost_matches_c() {
    let mut r = Rng(0xc0ff_ee11);
    for _ in 0..4000 {
        let mut skip = [[0i32; 2]; 3];
        let mut skip_mode = [[0i32; 2]; 3];
        let mut skip_flat = [0i32; 6];
        let mut skip_mode_flat = [0i32; 6];
        for i in 0..3 {
            for j in 0..2 {
                skip[i][j] = r.rate();
                skip_mode[i][j] = r.rate();
                skip_flat[i * 2 + j] = skip[i][j];
                skip_mode_flat[i * 2 + j] = skip_mode[i][j];
            }
        }
        let t = rfull::SkipFacBits { skip, skip_mode };

        let d: [u64; 12] = core::array::from_fn(|_| r.below(500_000));
        let dist = rfull::FullDist {
            y: rfull::PlaneDist {
                ssd_nonskip: d[0],
                ssd_skip: d[1],
                ssim_nonskip: d[2],
                ssim_skip: d[3],
            },
            cb: rfull::PlaneDist {
                ssd_nonskip: d[4],
                ssd_skip: d[5],
                ssim_nonskip: d[6],
                ssim_skip: d[7],
            },
            cr: rfull::PlaneDist {
                ssd_nonskip: d[8],
                ssd_skip: d[9],
                ssim_nonskip: d[10],
                ssim_skip: d[11],
            },
        };

        let mut f = [0i32; cref::FULL_FIELDS];
        f[0] = r.below(3) as i32; // skip_coeff_ctx
        f[1] = r.below(3) as i32; // skip_mode_ctx
        f[2] = i32::from(r.flag()); // update_full_cost_ssim
        f[3] = i32::from(r.flag()); // shut_fast_rate
        f[4] = 0; // tx_mode_select — see the module doc
        f[5] = i32::from(r.flag()); // lossless_segment
        f[6] = i32::from(r.flag()); // blk_skip_decision
        f[7] = i32::from(r.flag()); // block_has_coeff
        f[8] = i32::from(r.flag()); // is_inter_mode
        f[9] = i32::from(r.flag()); // skip_mode_allowed
        f[10] = BlockSize::Block16x16.as_index() as i32;

        let ycb = r.below(200_000);
        let cbcb = r.below(200_000);
        let crcb = r.below(200_000);
        let lambda = 1 + r.below(60_000);

        let c = cref::full_cost(&f, &d, &skip_flat, &skip_mode_flat, ycb, cbcb, crcb, lambda);

        let inputs = rfull::FullCostInputs {
            skip_coeff_ctx: f[0] as usize,
            skip_mode_ctx: f[1] as usize,
            update_full_cost_ssim: f[2] != 0,
            shut_fast_rate: f[3] != 0,
            tx_mode_select: f[4] != 0,
            lossless_segment: f[5] != 0,
            blk_skip_decision: f[6] != 0,
            block_has_coeff: f[7] != 0,
            is_inter_mode: f[8] != 0,
            skip_mode_allowed: f[9] != 0,
            non_skip_tx_size_bits: 0,
            skip_tx_size_bits: 0,
            fast_rate: 0,
        };
        let got = rfull::full_cost(&inputs, &dist, ycb, cbcb, crcb, lambda, &t);

        assert_eq!(got.cost, c[0], "cost, f={f:?}");
        assert_eq!(got.total_rate, c[1], "total_rate, f={f:?}");
        assert_eq!(u64::from(got.full_dist_u32()), c[2], "full_dist, f={f:?}");
        assert_eq!(
            got.full_cost_ssim.unwrap_or(0),
            c[3],
            "full_cost_ssim, f={f:?}"
        );
        // C reports the candidate buffer's post-state; the port reports the
        // two decisions that produce it.
        let port_has_coeff = inputs.block_has_coeff && !got.forced_coeff_skip && !got.skip_mode;
        assert_eq!(u64::from(port_has_coeff), c[4], "block_has_coeff, f={f:?}");
        assert_eq!(u64::from(got.skip_mode), c[5], "skip_mode, f={f:?}");
    }
}

#[test]
fn full_cost_pd0_matches_c() {
    let mut r = Rng(0x7777_1111);
    for _ in 0..4000 {
        let ycb = r.below(200_000);
        let dist = r.below(1_000_000);
        let skip0 = r.rate();
        let part0 = r.rate();
        let lambda = 1 + r.below(60_000);
        let c = cref::full_cost_pd0(ycb, dist, skip0, part0, lambda);
        let p = rfull::full_cost_pd0(ycb, dist, part0, skip0, lambda);
        assert_eq!(c, p);
    }
}

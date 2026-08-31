//! Traced vectors for the `static` writers of `entropy_coding.c`'s inter
//! group — `write_is_inter`, `write_ref_frames`, `write_inter_mode`,
//! `write_inter_compound_mode`, `write_drl_idx`, `encode_skip_mode_av1`,
//! `write_motion_mode`, `write_mb_interp_filter`,
//! `encode_intra_luma_mode_nonkey_av1`, `write_global_motion{,_params}` and
//! `write_sgrproj_filter`.
//!
//! **Evidence tier 4** (`docs/WORKING-ON-THIS.md` §4) — hand-derived vectors
//! traced against the C source, and it says so because tier 1 is
//! STRUCTURALLY unavailable for these: they are `static` in
//! `entropy_coding.c`, and `shims/ref_shims.c` #includes headers only and
//! never compiles that translation unit, so no shim can take their address.
//! A shim that re-transcribed their bit trees would be the "transcribed
//! oracle agreeing with transcribed code" §4 forbids.
//!
//! What IS gated at tier 1 elsewhere, and therefore deliberately not
//! re-derived here (`tests/c_parity_entropy_inter.rs`): every prediction
//! context, every CDF selector's `[ctx][slot]` row, every default CDF table,
//! and `svt_aom_wb_write_signed_primitive_refsubexpfin` — which is the whole
//! `aom_wb_write_primitive_{refsubexpfin,subexpfin,quniform}` stack, so those
//! three need no separate vector. The contexts are therefore COMPUTED by the
//! (tier-1-gated) port functions inside each expectation; what is hand-read
//! from C here is only the BRANCH STRUCTURE — which symbols, in which order,
//! against which table and slot.
//!
//! Method: each case replays a hand-written symbol sequence through a second
//! writer seeded with the same CDFs and compares the coded BYTES. Because the
//! CDFs adapt as they are written, byte equality pins the symbol values, their
//! order, their count and their CDF rows simultaneously; every case below is
//! additionally mutation-checked in the `sequence_comparison_is_sensitive`
//! test, which proves a wrong sequence produces different bytes.

use svtav1_encoder::entropy::context::FrameContext;
use svtav1_encoder::entropy::obu::BitWriter;
use svtav1_encoder::entropy::writer::AomWriter;
use svtav1_encoder::port_entropy_inter as p;
use svtav1_encoder::port_entropy_inter::modes::{DrlBlock, MotionMode, TransformationType as T};
use svtav1_encoder::port_entropy_inter::refframe::{
    RefFrameBlock, ReferenceMode, TOTAL_REFS_PER_FRAME,
};
use svtav1_encoder::port_entropy_inter::{InterCdfs, NeighborMi, Neighbors};
use svtav1_types::block::BlockSize;

/// Which CDF table an expected symbol is coded against.
#[derive(Clone, Copy, Debug)]
enum Tab {
    IntraInter,
    CompInter,
    CompRefType,
    UniCompRef(usize),
    CompRef(usize),
    CompBwdRef(usize),
    SingleRef(usize),
    NewMv,
    ZeroMv,
    RefMv,
    Drl,
    InterCompound,
    SkipMode,
    Obmc,
    MotionMode,
    SwitchableInterp,
    YMode,
    AngleDelta,
}

/// One expected symbol: table, row (the context), value.
#[derive(Clone, Copy, Debug)]
struct E {
    tab: Tab,
    row: usize,
    sym: usize,
}

/// One coded bit of a ref-frame tree: `(cdf slot, symbol)`.
type Bit = (usize, usize);

fn e(tab: Tab, row: usize, sym: usize) -> E {
    E { tab, row, sym }
}

/// Replay a hand-written symbol sequence through a fresh writer.
fn replay(exp: &[E]) -> Trace {
    let mut w = AomWriter::new(4096);
    let mut fc = FrameContext::new_default();
    let mut ic = InterCdfs::new_default();
    for it in exp {
        match it.tab {
            Tab::IntraInter => w.write_symbol(it.sym, &mut fc.intra_inter_cdf[it.row], 2),
            Tab::CompInter => w.write_symbol(it.sym, &mut fc.comp_inter_cdf[it.row], 2),
            Tab::CompRefType => w.write_symbol(it.sym, &mut ic.comp_ref_type_cdf[it.row], 2),
            Tab::UniCompRef(s) => w.write_symbol(it.sym, &mut ic.uni_comp_ref_cdf[it.row][s], 2),
            Tab::CompRef(s) => w.write_symbol(it.sym, &mut fc.comp_ref_cdf[it.row][s], 2),
            Tab::CompBwdRef(s) => w.write_symbol(it.sym, &mut ic.comp_bwdref_cdf[it.row][s], 2),
            Tab::SingleRef(s) => w.write_symbol(it.sym, &mut fc.single_ref_cdf[it.row][s], 2),
            Tab::NewMv => w.write_symbol(it.sym, &mut ic.newmv_cdf[it.row], 2),
            Tab::ZeroMv => w.write_symbol(it.sym, &mut ic.zeromv_cdf[it.row], 2),
            Tab::RefMv => w.write_symbol(it.sym, &mut ic.refmv_cdf[it.row], 2),
            Tab::Drl => w.write_symbol(it.sym, &mut ic.drl_cdf[it.row], 2),
            Tab::InterCompound => {
                w.write_symbol(it.sym, &mut ic.inter_compound_mode_cdf[it.row], 8)
            }
            Tab::SkipMode => w.write_symbol(it.sym, &mut ic.skip_mode_cdf[it.row], 2),
            Tab::Obmc => w.write_symbol(it.sym, &mut ic.obmc_cdf[it.row], 2),
            Tab::MotionMode => w.write_symbol(it.sym, &mut ic.motion_mode_cdf[it.row], 3),
            Tab::SwitchableInterp => {
                w.write_symbol(it.sym, &mut ic.switchable_interp_cdf[it.row], 3)
            }
            Tab::YMode => w.write_symbol(it.sym, &mut fc.y_mode_cdf[it.row], 13),
            Tab::AngleDelta => w.write_symbol(it.sym, &mut fc.angle_delta_cdf[it.row], 7),
        }
    }
    Trace::of(&mut w)
}

/// The coded bytes PLUS the arithmetic coder's internal state.
///
/// Bytes alone are not a sufficient comparison: the range coder can absorb a
/// dropped TRAILING high-probability symbol without changing a single output
/// byte (measured — a 12-symbol sequence and its 11-symbol prefix both coded
/// to `[15, 244]`). The `(low, rng, cnt, offs)` window is what actually
/// distinguishes them, so every case compares the full state.
#[derive(Debug, PartialEq, Eq)]
struct Trace {
    bytes: Vec<u8>,
    low: u64,
    rng: u16,
    cnt: i16,
    offs: usize,
}

impl Trace {
    fn of(w: &mut AomWriter) -> Self {
        let low = w.ec.low();
        let rng = w.ec.rng_val();
        let cnt = w.ec.cnt_val();
        let offs = w.ec.bytes_written();
        Self {
            bytes: w.done().to_vec(),
            low,
            rng,
            cnt,
            offs,
        }
    }
}

/// Run the writer under test with a fresh context.
fn run<F: FnOnce(&mut AomWriter, &mut FrameContext, &mut InterCdfs)>(f: F) -> Trace {
    let mut w = AomWriter::new(4096);
    let mut fc = FrameContext::new_default();
    let mut ic = InterCdfs::new_default();
    f(&mut w, &mut fc, &mut ic);
    Trace::of(&mut w)
}

/// A fixed neighbour pair, chosen so the ref counts are non-degenerate (one
/// forward single-ref above, one bidirectional-compound left) — an all-zero
/// count array would make every context 1 and hide a slot mix-up.
fn nb_fixture() -> Neighbors {
    Neighbors {
        above: Some(NeighborMi {
            mode: 16, // NEWMV
            ref_frame: [1, -1],
            interp_filters: (1 << 16) | 2,
            use_intrabc: false,
            skip_mode: true,
            comp_group_idx: 0,
            compound_idx: 1,
            bsize: BlockSize::Block16x16 as u8,
        }),
        left: Some(NeighborMi {
            mode: 24, // NEW_NEWMV
            ref_frame: [3, 7],
            interp_filters: 0,
            use_intrabc: false,
            skip_mode: false,
            comp_group_idx: 1,
            compound_idx: 0,
            bsize: BlockSize::Block32x32 as u8,
        }),
        up_available: true,
        left_available: true,
    }
}

fn counts_fixture(nb: &Neighbors) -> [u8; TOTAL_REFS_PER_FRAME] {
    let c = p::refframe::collect_neighbors_ref_counts(nb);
    assert!(
        c.iter().filter(|&&v| v != 0).count() >= 3,
        "the fixture must produce a non-degenerate count array, got {c:?}"
    );
    c
}

// ---- write_is_inter (entropy_coding.c:1147) ----

#[test]
fn write_is_inter_traced() {
    let nb = nb_fixture();
    let ctx = p::intra_inter_context(&nb);
    for is_inter in [false, true] {
        let got = run(|w, fc, _| p::modes::write_is_inter(w, fc, &nb, is_inter));
        let want = replay(&[e(Tab::IntraInter, ctx, usize::from(is_inter))]);
        assert_eq!(got, want, "write_is_inter({is_inter})");
    }
}

// ---- write_ref_frames (entropy_coding.c:2098) ----

/// The single-ref half of the tree, one row per reference. Read straight off
/// C's `else` arm at :2158-2178: p1 splits fwd/bwd, then {p2, p6} on the
/// backward side and {p3, p4|p5} on the forward side.
#[test]
fn write_ref_frames_single_ref_traced() {
    let nb = nb_fixture();
    let counts = counts_fixture(&nb);
    let sr = |n: usize| {
        let (c, s) = p::refframe::pred_cdf_single_ref(&counts, n);
        (c, s)
    };
    let comp_inter_ctx = p::refframe::reference_mode_context(&nb);

    // (ref, the bits after the comp_inter flag)
    let cases: [(i8, Vec<Bit>); 7] = [
        // LAST: p1=0, p3=0, p4=0
        (1, vec![(1, 0), (3, 0), (4, 0)]),
        // LAST2: p1=0, p3=0, p4=1
        (2, vec![(1, 0), (3, 0), (4, 1)]),
        // LAST3: p1=0, p3=1, p5=0
        (3, vec![(1, 0), (3, 1), (5, 0)]),
        // GOLDEN: p1=0, p3=1, p5=1
        (4, vec![(1, 0), (3, 1), (5, 1)]),
        // BWDREF: p1=1, p2=0, p6=0
        (5, vec![(1, 1), (2, 0), (6, 0)]),
        // ALTREF2: p1=1, p2=0, p6=1
        (6, vec![(1, 1), (2, 0), (6, 1)]),
        // ALTREF: p1=1, p2=1 — and NO p6
        (7, vec![(1, 1), (2, 1)]),
    ];

    for (rf, bits) in cases {
        let blk = RefFrameBlock {
            ref_frame: [rf, -1],
            bsize: BlockSize::Block16x16,
        };
        let got = run(|w, fc, ic| {
            p::refframe::write_ref_frames(w, fc, ic, &nb, &counts, ReferenceMode::Select, &blk)
        });
        let mut exp = vec![e(Tab::CompInter, comp_inter_ctx, 0)];
        for (n, sym) in bits {
            let (c, s) = sr(n);
            exp.push(E {
                tab: Tab::SingleRef(s),
                row: c,
                sym,
            });
        }
        assert_eq!(got, replay(&exp), "single ref {rf}");
    }
}

/// The bidirectional-compound arm, C :2136-2155.
#[test]
fn write_ref_frames_bidir_compound_traced() {
    let nb = nb_fixture();
    let counts = counts_fixture(&nb);
    let comp_inter_ctx = p::refframe::reference_mode_context(&nb);
    let crt_ctx = p::refframe::comp_reference_type_context(&nb);
    let cr = |n: usize| p::refframe::pred_cdf_comp_ref(&counts, n);
    let cb = |n: usize| p::refframe::pred_cdf_comp_bwdref(&counts, n);

    // (ref pair, forward bits as (slot, sym), backward bits as (slot, sym))
    let cases: [([i8; 2], Vec<Bit>, Vec<Bit>); 4] = [
        // LAST + BWDREF: bit=0 -> p1 (=LAST2? no); bwd: bit=0 -> p1 (=ALTREF2? no)
        ([1, 5], vec![(0, 0), (1, 0)], vec![(0, 0), (1, 0)]),
        // LAST2 + BWDREF: bit=0 -> p1 = 1
        ([2, 5], vec![(0, 0), (1, 1)], vec![(0, 0), (1, 0)]),
        // LAST3 + ALTREF2: bit=1 -> p2 = 0 (not GOLDEN); bwd bit=0 -> p1 = 1
        ([3, 6], vec![(0, 1), (2, 0)], vec![(0, 0), (1, 1)]),
        // GOLDEN + ALTREF: bit=1 -> p2 = 1; bwd bit=1, NO p1
        ([4, 7], vec![(0, 1), (2, 1)], vec![(0, 1)]),
    ];

    for (rf, fwd, bwd) in cases {
        let blk = RefFrameBlock {
            ref_frame: rf,
            bsize: BlockSize::Block16x16,
        };
        assert!(!blk.has_uni_comp_refs(), "{rf:?} must be bidirectional");
        let got = run(|w, fc, ic| {
            p::refframe::write_ref_frames(w, fc, ic, &nb, &counts, ReferenceMode::Select, &blk)
        });
        let mut exp = vec![
            e(Tab::CompInter, comp_inter_ctx, 1),
            e(Tab::CompRefType, crt_ctx, 1), // BIDIR_COMP_REFERENCE
        ];
        for (slot, sym) in fwd {
            let (c, s) = cr(slot);
            exp.push(E {
                tab: Tab::CompRef(s),
                row: c,
                sym,
            });
        }
        for (slot, sym) in bwd {
            let (c, s) = cb(slot);
            exp.push(E {
                tab: Tab::CompBwdRef(s),
                row: c,
                sym,
            });
        }
        assert_eq!(got, replay(&exp), "bidir compound {rf:?}");
    }
}

/// The unidirectional-compound arm, C :2117-2135 — note it RETURNS after the
/// tree, so no backward bits follow.
#[test]
fn write_ref_frames_unidir_compound_traced() {
    let nb = nb_fixture();
    let counts = counts_fixture(&nb);
    let comp_inter_ctx = p::refframe::reference_mode_context(&nb);
    let crt_ctx = p::refframe::comp_reference_type_context(&nb);
    let uc = |n: usize| p::refframe::pred_cdf_uni_comp_ref(&counts, n);

    let cases: [([i8; 2], Vec<Bit>); 4] = [
        ([1, 2], vec![(0, 0), (1, 0)]),         // LAST,LAST2
        ([1, 3], vec![(0, 0), (1, 1), (2, 0)]), // LAST,LAST3
        ([1, 4], vec![(0, 0), (1, 1), (2, 1)]), // LAST,GOLDEN
        ([5, 7], vec![(0, 1)]),                 // BWDREF,ALTREF — bit0=1, done
    ];

    for (rf, bits) in cases {
        let blk = RefFrameBlock {
            ref_frame: rf,
            bsize: BlockSize::Block16x16,
        };
        assert!(blk.has_uni_comp_refs(), "{rf:?} must be unidirectional");
        let got = run(|w, fc, ic| {
            p::refframe::write_ref_frames(w, fc, ic, &nb, &counts, ReferenceMode::Select, &blk)
        });
        let mut exp = vec![
            e(Tab::CompInter, comp_inter_ctx, 1),
            e(Tab::CompRefType, crt_ctx, 0), // UNIDIR_COMP_REFERENCE
        ];
        for (slot, sym) in bits {
            let (c, s) = uc(slot);
            exp.push(E {
                tab: Tab::UniCompRef(s),
                row: c,
                sym,
            });
        }
        assert_eq!(got, replay(&exp), "unidir compound {rf:?}");
    }
}

/// The `comp_inter` flag is coded ONLY under `REFERENCE_MODE_SELECT` AND
/// `is_comp_ref_allowed(bsize)` (C :2103-2106). Both gates matter: a 4x4
/// block never codes it even in SELECT mode.
#[test]
fn write_ref_frames_comp_inter_flag_gates_traced() {
    let nb = nb_fixture();
    let counts = counts_fixture(&nb);
    let (c1, s1) = p::refframe::pred_cdf_single_ref(&counts, 1);
    let (c3, s3) = p::refframe::pred_cdf_single_ref(&counts, 3);
    let (c4, s4) = p::refframe::pred_cdf_single_ref(&counts, 4);
    let tail = [
        E {
            tab: Tab::SingleRef(s1),
            row: c1,
            sym: 0,
        },
        E {
            tab: Tab::SingleRef(s3),
            row: c3,
            sym: 0,
        },
        E {
            tab: Tab::SingleRef(s4),
            row: c4,
            sym: 0,
        },
    ];

    for (mode, bsize, flag) in [
        (ReferenceMode::Select, BlockSize::Block16x16, true),
        (ReferenceMode::Select, BlockSize::Block4x4, false),
        (ReferenceMode::Select, BlockSize::Block4x16, false),
        (ReferenceMode::Single, BlockSize::Block16x16, false),
    ] {
        let blk = RefFrameBlock {
            ref_frame: [1, -1],
            bsize,
        };
        let got =
            run(|w, fc, ic| p::refframe::write_ref_frames(w, fc, ic, &nb, &counts, mode, &blk));
        let mut exp = Vec::new();
        if flag {
            exp.push(e(
                Tab::CompInter,
                p::refframe::reference_mode_context(&nb),
                0,
            ));
        }
        exp.extend_from_slice(&tail);
        assert_eq!(
            got,
            replay(&exp),
            "{mode:?} {bsize:?} (flag expected: {flag})"
        );
    }
}

// ---- write_inter_mode / write_inter_compound_mode ----

#[test]
fn write_inter_mode_traced() {
    // mode_ctx packs three contexts: newmv = ctx & 7, zeromv = (ctx>>3) & 1,
    // refmv = (ctx>>4) & 15 (C :1386-1399). C asserts `newmv_ctx <
    // NEWMV_MODE_CONTEXTS` (6) and `refmv_ctx < REFMV_MODE_CONTEXTS` (6), so
    // the masks are WIDER than the tables and a raw 0..=255 sweep would feed
    // C an out-of-range row too. The sweep therefore builds mode_ctx from
    // in-range fields, which is what `svt_aom_mode_context_analyzer` produces.
    let ctxs: Vec<i16> = (0..6i16)
        .flat_map(|nc| {
            (0..2i16).flat_map(move |zc| (0..6i16).map(move |rc| nc | (zc << 3) | (rc << 4)))
        })
        .collect();
    for mode_ctx in ctxs {
        let nc = (mode_ctx & 7) as usize;
        let zc = ((mode_ctx >> 3) & 1) as usize;
        let rc = ((mode_ctx >> 4) & 15) as usize;
        let cases: [(u8, Vec<E>); 4] = [
            (16, vec![e(Tab::NewMv, nc, 0)]), // NEWMV: one symbol, done
            (
                15, // GLOBALMV
                vec![e(Tab::NewMv, nc, 1), e(Tab::ZeroMv, zc, 0)],
            ),
            (
                13, // NEARESTMV
                vec![
                    e(Tab::NewMv, nc, 1),
                    e(Tab::ZeroMv, zc, 1),
                    e(Tab::RefMv, rc, 0),
                ],
            ),
            (
                14, // NEARMV
                vec![
                    e(Tab::NewMv, nc, 1),
                    e(Tab::ZeroMv, zc, 1),
                    e(Tab::RefMv, rc, 1),
                ],
            ),
        ];
        for (mode, exp) in cases {
            let got = run(|w, _, ic| p::modes::write_inter_mode(w, ic, mode, mode_ctx));
            assert_eq!(got, replay(&exp), "mode {mode} ctx {mode_ctx}");
        }
    }
}

#[test]
fn write_inter_compound_mode_traced() {
    for mode_ctx in 0i16..8 {
        for mode in 17u8..=24 {
            let got = run(|w, _, ic| p::modes::write_inter_compound_mode(w, ic, mode, mode_ctx));
            let want = replay(&[e(
                Tab::InterCompound,
                mode_ctx as usize,
                (mode - 17) as usize,
            )]);
            assert_eq!(got, want, "compound mode {mode} ctx {mode_ctx}");
        }
    }
}

// ---- write_drl_idx (entropy_coding.c:1404) ----

#[test]
fn write_drl_idx_traced() {
    struct Case {
        name: &'static str,
        mode: u8,
        blk: DrlBlock,
        exp: Vec<E>,
    }
    let cases = vec![
        Case {
            name: "NEWMV, drl_index 0, first ctx present -> one bit then return",
            mode: 16,
            blk: DrlBlock {
                drl_ctx: [2, 1],
                drl_ctx_near: [0, 0],
                drl_index: 0,
            },
            exp: vec![e(Tab::Drl, 2, 0)],
        },
        Case {
            name: "NEWMV, drl_index 1 -> bit 1 then bit 0",
            mode: 16,
            blk: DrlBlock {
                drl_ctx: [1, 2],
                drl_ctx_near: [0, 0],
                drl_index: 1,
            },
            exp: vec![e(Tab::Drl, 1, 1), e(Tab::Drl, 2, 0)],
        },
        Case {
            name: "NEWMV, first ctx == -1 is SKIPPED, not coded as 0",
            mode: 16,
            blk: DrlBlock {
                drl_ctx: [-1, 0],
                drl_ctx_near: [0, 0],
                drl_index: 1,
            },
            exp: vec![e(Tab::Drl, 0, 0)],
        },
        Case {
            name: "NEWMV, drl_index 2 -> both bits are 1 and the loop ends",
            mode: 16,
            blk: DrlBlock {
                drl_ctx: [0, 1],
                drl_ctx_near: [0, 0],
                drl_index: 2,
            },
            exp: vec![e(Tab::Drl, 0, 1), e(Tab::Drl, 1, 1)],
        },
        Case {
            name: "NEW_NEWMV takes the same new_mv arm as NEWMV",
            mode: 24,
            blk: DrlBlock {
                drl_ctx: [2, 1],
                drl_ctx_near: [0, 0],
                drl_index: 0,
            },
            exp: vec![e(Tab::Drl, 2, 0)],
        },
        Case {
            name: "NEARMV uses drl_ctx_near with the idx-1 offset",
            mode: 14,
            blk: DrlBlock {
                drl_ctx: [0, 0],
                drl_ctx_near: [1, 2],
                drl_index: 0,
            },
            exp: vec![e(Tab::Drl, 1, 0)],
        },
        Case {
            name: "NEARMV, drl_index 1 -> both near positions",
            mode: 14,
            blk: DrlBlock {
                drl_ctx: [0, 0],
                drl_ctx_near: [1, 2],
                drl_index: 1,
            },
            exp: vec![e(Tab::Drl, 1, 1), e(Tab::Drl, 2, 0)],
        },
        Case {
            name: "NEARESTMV codes NOTHING (neither predicate)",
            mode: 13,
            blk: DrlBlock {
                drl_ctx: [1, 1],
                drl_ctx_near: [1, 1],
                drl_index: 0,
            },
            exp: vec![],
        },
        Case {
            // The trap `inter_mv_code.rs` records: NEAREST_NEWMV writes an MV
            // but NO drl index, because have_nearmv_in_inter_mode is false
            // for it.
            name: "NEAREST_NEWMV codes NOTHING even though it codes an MV",
            mode: 19,
            blk: DrlBlock {
                drl_ctx: [1, 1],
                drl_ctx_near: [1, 1],
                drl_index: 0,
            },
            exp: vec![],
        },
        Case {
            name: "NEAR_NEWMV is in the near arm",
            mode: 21,
            blk: DrlBlock {
                drl_ctx: [0, 0],
                drl_ctx_near: [2, 0],
                drl_index: 0,
            },
            exp: vec![e(Tab::Drl, 2, 0)],
        },
    ];
    for c in cases {
        let got = run(|w, _, ic| p::modes::write_drl_idx(w, ic, c.mode, &c.blk));
        assert_eq!(got, replay(&c.exp), "{}", c.name);
    }
}

// ---- encode_skip_mode_av1 (entropy_coding.c:1109) ----

#[test]
fn encode_skip_mode_traced() {
    let mut nb = nb_fixture();
    for (a_sm, l_sm, want_ctx) in [(false, false, 0), (true, false, 1), (true, true, 2)] {
        nb.above.as_mut().unwrap().skip_mode = a_sm;
        nb.left.as_mut().unwrap().skip_mode = l_sm;
        assert_eq!(p::modes::skip_mode_context(&nb), want_ctx);
        for flag in [false, true] {
            let got = run(|w, _, ic| p::modes::encode_skip_mode(w, ic, &nb, flag));
            let want = replay(&[e(Tab::SkipMode, want_ctx, usize::from(flag))]);
            assert_eq!(got, want, "skip_mode {flag} ctx {want_ctx}");
        }
    }
}

// ---- write_motion_mode (entropy_coding.c:1198) ----

#[test]
fn write_motion_mode_traced() {
    let bsize = BlockSize::Block16x16;
    let row = bsize.as_index();
    // SIMPLE_TRANSLATION allowed -> no symbol at all.
    for mm in [
        MotionMode::SimpleTranslation,
        MotionMode::ObmcCausal,
        MotionMode::WarpedCausal,
    ] {
        let got = run(|w, _, ic| {
            p::modes::write_motion_mode(w, ic, bsize, mm, MotionMode::SimpleTranslation)
        });
        assert_eq!(got, replay(&[]), "SIMPLE_TRANSLATION must code nothing");
    }
    // OBMC_CAUSAL allowed -> one binary obmc_cdf symbol.
    for (mm, sym) in [
        (MotionMode::SimpleTranslation, 0),
        (MotionMode::ObmcCausal, 1),
    ] {
        let got =
            run(|w, _, ic| p::modes::write_motion_mode(w, ic, bsize, mm, MotionMode::ObmcCausal));
        assert_eq!(got, replay(&[e(Tab::Obmc, row, sym)]), "obmc arm {mm:?}");
    }
    // WARPED_CAUSAL allowed -> one 3-symbol motion_mode_cdf symbol.
    for mm in [
        MotionMode::SimpleTranslation,
        MotionMode::ObmcCausal,
        MotionMode::WarpedCausal,
    ] {
        let got =
            run(|w, _, ic| p::modes::write_motion_mode(w, ic, bsize, mm, MotionMode::WarpedCausal));
        assert_eq!(
            got,
            replay(&[e(Tab::MotionMode, row, mm as usize)]),
            "warped arm {mm:?}"
        );
    }
}

// ---- write_mb_interp_filter (entropy_coding.c:1608) ----

#[test]
fn write_mb_interp_filter_traced() {
    let nb = nb_fixture();
    let bsize = BlockSize::Block16x16;
    let gm = [T::Identity; 8];
    let filters: u32 = (2 << 16) | 1; // x = MULTITAP_SHARP, y = EIGHTTAP_SMOOTH
    let (rf0, rf1) = (1i8, -1i8);
    let ctx0 = p::interp::pred_context_switchable_interp(rf0, rf1, &nb, 0);
    let ctx1 = p::interp::pred_context_switchable_interp(rf0, rf1, &nb, 1);

    // Not SWITCHABLE at the frame level -> nothing.
    let got = run(|w, _, ic| {
        p::interp::write_mb_interp_filter(
            w,
            ic,
            &nb,
            0,
            true,
            bsize,
            rf0,
            rf1,
            16,
            false,
            MotionMode::SimpleTranslation,
            filters,
            &gm,
        )
    });
    assert_eq!(
        got,
        replay(&[]),
        "non-switchable frame filter codes nothing"
    );

    // skip_mode -> nothing (av1_is_interp_needed's first early-out).
    let got = run(|w, _, ic| {
        p::interp::write_mb_interp_filter(
            w,
            ic,
            &nb,
            p::interp::SWITCHABLE,
            true,
            bsize,
            rf0,
            rf1,
            16,
            true,
            MotionMode::SimpleTranslation,
            filters,
            &gm,
        )
    });
    assert_eq!(got, replay(&[]), "skip_mode codes no filter");

    // WARPED_CAUSAL -> nothing.
    let got = run(|w, _, ic| {
        p::interp::write_mb_interp_filter(
            w,
            ic,
            &nb,
            p::interp::SWITCHABLE,
            true,
            bsize,
            rf0,
            rf1,
            16,
            false,
            MotionMode::WarpedCausal,
            filters,
            &gm,
        )
    });
    assert_eq!(got, replay(&[]), "WARPED_CAUSAL codes no filter");

    // Non-translational global motion on a GLOBALMV block -> nothing.
    let mut gm_rz = [T::Identity; 8];
    gm_rz[1] = T::RotZoom;
    assert!(p::interp::is_nontrans_global_motion(
        15,
        bsize,
        [rf0, rf1],
        &gm_rz
    ));
    let got = run(|w, _, ic| {
        p::interp::write_mb_interp_filter(
            w,
            ic,
            &nb,
            p::interp::SWITCHABLE,
            true,
            bsize,
            rf0,
            rf1,
            15,
            false,
            MotionMode::SimpleTranslation,
            filters,
            &gm_rz,
        )
    });
    assert_eq!(got, replay(&[]), "non-translational GM codes no filter");

    // Dual filter OFF -> exactly one symbol, direction 0 (the Y filter).
    let got = run(|w, _, ic| {
        p::interp::write_mb_interp_filter(
            w,
            ic,
            &nb,
            p::interp::SWITCHABLE,
            false,
            bsize,
            rf0,
            rf1,
            16,
            false,
            MotionMode::SimpleTranslation,
            filters,
            &gm,
        )
    });
    assert_eq!(
        got,
        replay(&[e(Tab::SwitchableInterp, ctx0, 1)]),
        "dual filter off codes one symbol"
    );

    // Dual filter ON -> two symbols, dir 0 then dir 1.
    let got = run(|w, _, ic| {
        p::interp::write_mb_interp_filter(
            w,
            ic,
            &nb,
            p::interp::SWITCHABLE,
            true,
            bsize,
            rf0,
            rf1,
            16,
            false,
            MotionMode::SimpleTranslation,
            filters,
            &gm,
        )
    });
    assert_eq!(
        got,
        replay(&[
            e(Tab::SwitchableInterp, ctx0, 1),
            e(Tab::SwitchableInterp, ctx1, 2),
        ]),
        "dual filter on codes both directions"
    );
}

// ---- encode_intra_luma_mode_nonkey_av1 (entropy_coding.c:1046) ----

#[test]
fn encode_intra_luma_mode_nonkey_traced() {
    // (bsize, mode, angle_delta, expected symbols)
    let cases: [(BlockSize, u8, i8, bool); 6] = [
        // Below BLOCK_8X8 in enum order: no angle delta even for a
        // directional mode.
        (BlockSize::Block4x4, 1, -2, false),
        (BlockSize::Block4x8, 8, 3, false),
        // Non-directional: no angle delta.
        (BlockSize::Block16x16, 0, 0, false),
        (BlockSize::Block16x16, 12, 0, false),
        // Directional and >= BLOCK_8X8.
        (BlockSize::Block8x8, 1, -2, true),
        // The enum-ordinal quirk: BLOCK_4X16 is index 16, so `>= BLOCK_8X8`
        // (3) holds even though the block is 4 wide.
        (BlockSize::Block4x16, 5, 1, true),
    ];
    for (bsize, mode, delta, with_delta) in cases {
        let got = run(|w, fc, _| {
            p::modes::encode_intra_luma_mode_nonkey(w, fc, bsize, mode, mode, delta)
        });
        let group = p::modes::SIZE_GROUP_LOOKUP[bsize.as_index()] as usize;
        let mut exp = vec![e(Tab::YMode, group, mode as usize)];
        if with_delta {
            exp.push(e(
                Tab::AngleDelta,
                (mode - 1) as usize,
                (delta + 3) as usize,
            ));
        }
        assert_eq!(got, replay(&exp), "{bsize:?} mode {mode} delta {delta}");
    }
}

// ---- write_global_motion_params / write_global_motion ----

/// Append `bits` (MSB-first in `bytes`) to a bit writer.
fn append_bits(wb: &mut BitWriter, bits: usize, bytes: &[u8]) {
    for i in 0..bits {
        let b = (bytes[i / 8] >> (7 - (i % 8))) & 1;
        wb.write_bit(b != 0);
    }
}

/// The coefficient writes are re-created here from the TIER-1-gated
/// `svt_aom_wb_write_signed_primitive_refsubexpfin`, so only the type tree and
/// the per-type coefficient SELECTION are hand-derived.
fn expect_gm_params(
    params: &p::gm::WarpParams,
    ref_params: &p::gm::WarpParams,
    allow_hp: bool,
) -> BitWriter {
    use svtav1_cref::entropy_inter as cref;
    let mut wb = BitWriter::new();
    let ty = params.wmtype;
    wb.write_bit(ty != T::Identity);
    if ty != T::Identity {
        wb.write_bit(ty == T::RotZoom);
        if ty != T::RotZoom {
            wb.write_bit(ty == T::Translation);
        }
    }
    let alpha_n = p::gm::GM_ALPHA_MAX as i32 + 1;
    let k = p::gm::SUBEXPFIN_K as i32;
    let sgn = |wb: &mut BitWriter, n: i32, r: i32, v: i32| {
        let (bits, bytes) = cref::wb_signed_refsubexpfin(n, k, r, v);
        append_bits(wb, bits, &bytes);
    };
    if ty >= T::RotZoom {
        let d = p::gm::GM_ALPHA_PREC_DIFF;
        let o = 1 << p::gm::GM_ALPHA_PREC_BITS;
        sgn(
            &mut wb,
            alpha_n,
            (ref_params.wmmat[2] >> d) - o,
            (params.wmmat[2] >> d) - o,
        );
        sgn(
            &mut wb,
            alpha_n,
            ref_params.wmmat[3] >> d,
            params.wmmat[3] >> d,
        );
    }
    if ty >= T::Affine {
        let d = p::gm::GM_ALPHA_PREC_DIFF;
        let o = 1 << p::gm::GM_ALPHA_PREC_BITS;
        sgn(
            &mut wb,
            alpha_n,
            ref_params.wmmat[4] >> d,
            params.wmmat[4] >> d,
        );
        sgn(
            &mut wb,
            alpha_n,
            (ref_params.wmmat[5] >> d) - o,
            (params.wmmat[5] >> d) - o,
        );
    }
    if ty >= T::Translation {
        let trans_bits = if ty == T::Translation {
            p::gm::GM_ABS_TRANS_ONLY_BITS - i32::from(!allow_hp)
        } else {
            p::gm::GM_ABS_TRANS_BITS
        };
        let d = if ty == T::Translation {
            p::gm::GM_TRANS_ONLY_PREC_DIFF + u32::from(!allow_hp)
        } else {
            p::gm::GM_TRANS_PREC_DIFF
        };
        let n = (1 << trans_bits) + 1;
        sgn(&mut wb, n, ref_params.wmmat[0] >> d, params.wmmat[0] >> d);
        sgn(&mut wb, n, ref_params.wmmat[1] >> d, params.wmmat[1] >> d);
    }
    wb
}

fn warp(ty: T, wmmat: [i32; 6]) -> p::gm::WarpParams {
    p::gm::WarpParams { wmtype: ty, wmmat }
}

#[test]
fn write_global_motion_params_traced() {
    let one = 1 << p::gm::WARPEDMODEL_PREC_BITS;
    let cases = [
        warp(T::Identity, [0, 0, one, 0, 0, one]),
        warp(T::Translation, [512, -256, one, 0, 0, one]),
        warp(T::RotZoom, [1024, 2048, one + 4096, -8192, 0, one]),
        warp(T::Affine, [-1024, 64, one - 3000, 5000, -700, one + 900]),
    ];
    let mut lens = std::collections::BTreeSet::new();
    for p_ in cases {
        for r_ in cases {
            for allow_hp in [false, true] {
                let mut wb = BitWriter::new();
                p::gm::write_global_motion_params(&mut wb, &p_, &r_, allow_hp);
                let want = expect_gm_params(&p_, &r_, allow_hp);
                assert_eq!(
                    wb.bit_len(),
                    want.bit_len(),
                    "bit count ({:?} vs ref {:?}, hp={allow_hp})",
                    p_.wmtype,
                    r_.wmtype
                );
                assert_eq!(
                    wb.data(),
                    want.data(),
                    "bits ({:?} vs ref {:?}, hp={allow_hp})",
                    p_.wmtype,
                    r_.wmtype
                );
                lens.insert(wb.bit_len());
            }
        }
    }
    assert!(
        lens.len() >= 4,
        "the sweep must exercise several lengths, got {lens:?}"
    );
    assert!(
        lens.contains(&1),
        "IDENTITY must code exactly one bit; lengths seen {lens:?}"
    );
}

#[test]
fn write_global_motion_traced() {
    let one = 1 << p::gm::WARPEDMODEL_PREC_BITS;
    let mut gm = [p::gm::WarpParams::IDENTITY; 8];
    gm[1] = warp(T::Translation, [512, -256, one, 0, 0, one]);
    gm[4] = warp(T::RotZoom, [0, 0, one + 1000, 200, 0, one]);
    let mut refgm = [p::gm::WarpParams::IDENTITY; 8];
    refgm[1] = warp(T::Translation, [128, 0, one, 0, 0, one]);

    for primary_ref in [p::gm::PRIMARY_REF_NONE, 0u8] {
        let mut wb = BitWriter::new();
        p::gm::write_global_motion(&mut wb, &gm, &refgm, primary_ref, true);

        // C loops LAST..ALTREF (1..=7) and codes each against
        // ref_global_motion[frame] when primary_ref_frame != PRIMARY_REF_NONE,
        // else against default_warp_params.
        let mut want = BitWriter::new();
        for f in 1usize..=7 {
            let r = if primary_ref != p::gm::PRIMARY_REF_NONE {
                refgm[f]
            } else {
                p::gm::WarpParams::IDENTITY
            };
            let part = expect_gm_params(&gm[f], &r, true);
            append_bits(&mut want, part.bit_len(), part.data());
        }
        assert_eq!(wb.bit_len(), want.bit_len(), "primary_ref {primary_ref}");
        assert_eq!(wb.data(), want.data(), "primary_ref {primary_ref}");
    }

    // All-IDENTITY with no CDF continuation is the ONLY case where
    // entropy/obu.rs's seven hardcoded zero bits are right.
    let ident = [p::gm::WarpParams::IDENTITY; 8];
    let mut wb = BitWriter::new();
    p::gm::write_global_motion(&mut wb, &ident, &ident, p::gm::PRIMARY_REF_NONE, true);
    assert_eq!(wb.bit_len(), 7, "seven references, one IDENTITY bit each");
    assert_eq!(wb.data(), &[0u8], "all seven bits are zero");
}

// ---- write_sgrproj_filter (entropy_coding.c:4069) ----

#[test]
fn write_sgrproj_filter_traced() {
    use svtav1_encoder::entropy::lr::write_primitive_refsubexpfin;
    use svtav1_encoder::port_entropy_inter::gm::{
        SGRPROJ_PARAMS_BITS, SGRPROJ_PRJ_MAX0, SGRPROJ_PRJ_MAX1, SGRPROJ_PRJ_MIN0,
        SGRPROJ_PRJ_MIN1, SGRPROJ_PRJ_SUBEXP_K, SgrprojInfo,
    };

    // (r0 == 0, r1 == 0, info, ref)
    let cases = [
        (
            true,
            false,
            SgrprojInfo {
                ep: 9,
                xqd: [0, 40],
            },
            SgrprojInfo {
                ep: 0,
                xqd: [0, 31],
            },
        ),
        (
            false,
            true,
            SgrprojInfo {
                ep: 3,
                xqd: [-60, 0],
            },
            SgrprojInfo {
                ep: 1,
                xqd: [-96, 0],
            },
        ),
        (
            false,
            false,
            SgrprojInfo {
                ep: 15,
                xqd: [-20, 12],
            },
            SgrprojInfo {
                ep: 2,
                xqd: [4, -8],
            },
        ),
    ];

    for (r0z, r1z, info, refi) in cases {
        let mut ref_a = refi;
        let mut w = AomWriter::new(1024);
        p::gm::write_sgrproj_filter(&mut w, &info, &mut ref_a, r0z, r1z);
        let got = Trace::of(&mut w);

        // Hand-derived from C :4069-4099: the 4-bit ep literal, then the
        // r[0]==0 / r[1]==0 / both-nonzero coefficient selection.
        let mut w2 = AomWriter::new(1024);
        w2.write_literal(info.ep as u32, SGRPROJ_PARAMS_BITS);
        if r0z {
            write_primitive_refsubexpfin(
                &mut w2,
                (SGRPROJ_PRJ_MAX1 - SGRPROJ_PRJ_MIN1 + 1) as u16,
                SGRPROJ_PRJ_SUBEXP_K,
                (refi.xqd[1] - SGRPROJ_PRJ_MIN1) as u16,
                (info.xqd[1] - SGRPROJ_PRJ_MIN1) as u16,
            );
        } else if r1z {
            write_primitive_refsubexpfin(
                &mut w2,
                (SGRPROJ_PRJ_MAX0 - SGRPROJ_PRJ_MIN0 + 1) as u16,
                SGRPROJ_PRJ_SUBEXP_K,
                (refi.xqd[0] - SGRPROJ_PRJ_MIN0) as u16,
                (info.xqd[0] - SGRPROJ_PRJ_MIN0) as u16,
            );
        } else {
            write_primitive_refsubexpfin(
                &mut w2,
                (SGRPROJ_PRJ_MAX0 - SGRPROJ_PRJ_MIN0 + 1) as u16,
                SGRPROJ_PRJ_SUBEXP_K,
                (refi.xqd[0] - SGRPROJ_PRJ_MIN0) as u16,
                (info.xqd[0] - SGRPROJ_PRJ_MIN0) as u16,
            );
            write_primitive_refsubexpfin(
                &mut w2,
                (SGRPROJ_PRJ_MAX1 - SGRPROJ_PRJ_MIN1 + 1) as u16,
                SGRPROJ_PRJ_SUBEXP_K,
                (refi.xqd[1] - SGRPROJ_PRJ_MIN1) as u16,
                (info.xqd[1] - SGRPROJ_PRJ_MIN1) as u16,
            );
        }
        assert_eq!(got, Trace::of(&mut w2), "sgrproj ep {}", info.ep);

        // C's trailing svt_memcpy: the reference for the next RU is this one.
        assert_eq!(ref_a, info, "ref_sgrproj_info must be updated in place");
    }
}

// ---- the comparison itself must be able to fail ----

/// Anti-vacuity for the whole file: the byte comparison used above must
/// distinguish a changed SYMBOL, a changed CDF ROW, a changed ORDER and a
/// changed COUNT. If it could not, every assertion above would be empty.
#[test]
fn sequence_comparison_is_sensitive() {
    let base: Vec<E> = (0..12)
        .map(|i| e(Tab::SingleRef(i % 6), (i / 2) % 3, (i % 3) & 1))
        .collect();
    let b = replay(&base);

    let mut sym = base.clone();
    sym[1].sym = 0;
    assert_ne!(b, replay(&sym), "a changed symbol must change the trace");

    let mut row = base.clone();
    row[2].row = 0;
    assert_ne!(b, replay(&row), "a changed CDF row must change the trace");

    let mut ord = base.clone();
    ord.swap(0, 1);
    assert_ne!(b, replay(&ord), "a changed order must change the trace");

    let mut cnt = base.clone();
    cnt.pop();
    assert_ne!(
        b,
        replay(&cnt),
        "a changed symbol count must change the trace"
    );
    // …and record WHY the trace carries coder state: the bytes alone do NOT
    // separate these two.
    assert_eq!(
        b.bytes,
        replay(&cnt).bytes,
        "this is the measured case where the output BYTES are identical for a \n         12-symbol sequence and its 11-symbol prefix — if this ever stops \n         holding, the comment above `Trace` should be re-measured, not deleted"
    );

    assert_ne!(
        b,
        replay(&[]),
        "an empty sequence must differ from a nonempty one"
    );
}

// ---- write_frame_size_with_refs / get_ref_order_hint (:3238 / :3230) ----

#[test]
fn write_frame_size_with_refs_traced() {
    use svtav1_encoder::port_entropy_inter::framesize::{
        FrameSizeRefs, INVALID_ORDER_HINT, RefPic, get_ref_order_hint, write_frame_size_with_refs,
    };

    let mk = |oh: u32, w: u32, h: u32| RefPic {
        order_hint: oh,
        width: w,
        height: h,
    };
    // DPB slots 0..7 carry these order hints; LAST..ALTREF map onto slots
    // 2, -1, 3, 4, 5, 6, 7 — LAST2 is deliberately absent.
    let dpb = [90u32, 91, 20, 30, 40, 50, 60, 70];
    let idx = [2i32, -1, 3, 4, 5, 6, 7];

    // get_ref_order_hint: the absent slot returns the INVALID sentinel.
    let refs_probe = FrameSizeRefs {
        ref_dpb_index: idx,
        dpb_order_hint: dpb,
        list0: &[],
        list1: &[],
        cur_width: 64,
        cur_height: 64,
    };
    assert_eq!(get_ref_order_hint(&refs_probe, 1), 20, "LAST -> slot 2");
    assert_eq!(
        get_ref_order_hint(&refs_probe, 2),
        INVALID_ORDER_HINT,
        "LAST2 has no DPB slot"
    );
    assert_eq!(get_ref_order_hint(&refs_probe, 7), 70, "ALTREF -> slot 7");

    // 1. Nothing matches -> seven zero bits, then the explicit frame size.
    let refs = FrameSizeRefs {
        list0: &[mk(999, 64, 64)],
        ..refs_probe
    };
    let mut wb = BitWriter::new();
    let mut hit_superres = false;
    let mut hit_framesize = false;
    write_frame_size_with_refs(
        &mut wb,
        &refs,
        |_| hit_superres = true,
        |w| {
            hit_framesize = true;
            w.write_bit(true); // stand-in for write_frame_size's payload
        },
    );
    assert!(!hit_superres && hit_framesize, "no match must fall through");
    assert_eq!(wb.bit_len(), 8, "seven `found` bits plus the payload bit");
    assert_eq!(wb.data(), &[0b0000_0001], "all seven found bits are zero");

    // 2. LAST matches in list 0 -> ONE `1` bit, superres scale, and the loop
    //    short-circuits (no further `found` bits).
    let l0 = [mk(20, 64, 64)];
    let refs = FrameSizeRefs {
        list0: &l0,
        ..refs_probe
    };
    let mut wb = BitWriter::new();
    let mut hit_framesize = false;
    write_frame_size_with_refs(
        &mut wb,
        &refs,
        |w| w.write_bit(false),
        |_| hit_framesize = true,
    );
    assert!(!hit_framesize, "a match must not write an explicit size");
    assert_eq!(wb.bit_len(), 2, "one found bit plus the superres bit");
    assert_eq!(wb.data(), &[0b1000_0000]);

    // 3. A right order hint with the WRONG dimensions is not a match; the
    //    walk continues to LAST3 (slot 3, hint 30), which matches in LIST 1.
    let l0 = [mk(20, 32, 64)];
    let l1 = [mk(30, 64, 64)];
    let refs = FrameSizeRefs {
        list0: &l0,
        list1: &l1,
        ..refs_probe
    };
    let mut wb = BitWriter::new();
    write_frame_size_with_refs(&mut wb, &refs, |w| w.write_bit(true), |_| {});
    // LAST 0 (size mismatch), LAST2 0 (no slot), LAST3 1, then superres 1.
    assert_eq!(wb.bit_len(), 4);
    assert_eq!(wb.data(), &[0b0011_0000]);
}

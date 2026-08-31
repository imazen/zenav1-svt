//! Differential parity for INTER motion-vector entropy coding and MV rate —
//! campaign chunk C3 (`docs/INTER-ENCODE-PLAN.md`).
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4) for everything below
//! except where a test says otherwise in its own doc comment: each assertion
//! drives a REAL EXPORTED C symbol out of `libSvtAv1Enc.a` —
//! `svt_av1_encode_mv`, `svt_av1_get_mv_joint`, `svt_aom_estimate_mv_rate`,
//! `svt_av1_mv_bit_cost`, `svt_aom_have_newmv_in_inter_mode`,
//! `svt_av1_reset_cdf_symbol_counters`, `svt_aom_get_update_cdf_level_*` —
//! not a transcription of one.
//!
//! What separates this suite from the pre-existing `c_parity_mv.rs`:
//!
//! * `c_parity_mv.rs` drives a faithful C-side TRANSCRIPTION of
//!   `svt_av1_encode_mv` (`ref_encode_mv_seq`), always from the default
//!   context, always with `ref_mv == 0`, and compares BYTES only.
//! * This suite drives the real `svt_av1_encode_mv`, seeds RANDOMIZED
//!   `NmvContext`s, uses NONZERO reference MVs (so the `mv - ref`
//!   subtraction is actually exercised), sweeps `force_integer_mv` and
//!   `allow_update_cdf`, and compares the ADAPTED CDF STATE as well as the
//!   bytes — which is the half a bytes-only test cannot see.

use svtav1_cref as cref;
use svtav1_encoder::entropy::mv_coding::{
    CLASS0_SIZE, MV_CLASSES, MV_FP_SIZE, MV_JOINTS, MV_OFFSET_BITS, MvSubpelPrecision,
    NmvComponent, NmvContext,
};
use svtav1_encoder::entropy::writer::AomWriter;
use svtav1_encoder::inter_mv_code as imc;
use svtav1_encoder::intrabc;
use svtav1_types::motion::Mv;
use svtav1_types::prediction::PredictionMode;

// ---------------------------------------------------------------------------
// Test-local RNG + NmvContext (de)serialization in C struct-layout order
// ---------------------------------------------------------------------------

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn range_i32(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.below((hi - lo + 1) as u64) as i32)
    }
}

/// A random valid CDF in C layout: strictly decreasing ICDF values, the
/// structural 0 at `[nsymbs-1]`, a random adaptation counter at `[nsymbs]`.
fn random_cdf(rng: &mut Rng, out: &mut [u16]) {
    let nsymbs = out.len() - 1;
    loop {
        let mut cuts: Vec<u16> = (0..nsymbs - 1)
            .map(|_| 1 + rng.below(32766) as u16)
            .collect();
        cuts.sort_unstable_by(|a, b| b.cmp(a));
        cuts.dedup();
        if cuts.len() == nsymbs - 1 {
            out[..nsymbs - 1].copy_from_slice(&cuts);
            break;
        }
    }
    out[nsymbs - 1] = 0;
    out[nsymbs] = rng.below(33) as u16;
}

fn random_nmv_component(rng: &mut Rng) -> NmvComponent {
    let mut c = NmvComponent {
        classes_cdf: [0; MV_CLASSES + 1],
        class0_fp_cdf: [[0; MV_FP_SIZE + 1]; CLASS0_SIZE],
        fp_cdf: [0; MV_FP_SIZE + 1],
        sign_cdf: [0; 3],
        class0_hp_cdf: [0; 3],
        hp_cdf: [0; 3],
        class0_cdf: [0; CLASS0_SIZE + 1],
        bits_cdf: [[0; 3]; MV_OFFSET_BITS],
    };
    random_cdf(rng, &mut c.classes_cdf);
    for row in &mut c.class0_fp_cdf {
        random_cdf(rng, row);
    }
    random_cdf(rng, &mut c.fp_cdf);
    random_cdf(rng, &mut c.sign_cdf);
    random_cdf(rng, &mut c.class0_hp_cdf);
    random_cdf(rng, &mut c.hp_cdf);
    random_cdf(rng, &mut c.class0_cdf);
    for row in &mut c.bits_cdf {
        random_cdf(rng, row);
    }
    c
}

fn random_nmv_context(rng: &mut Rng) -> NmvContext {
    let mut ctx = NmvContext::default();
    random_cdf(rng, &mut ctx.joints_cdf);
    ctx.comps = [random_nmv_component(rng), random_nmv_component(rng)];
    ctx
}

/// The 143-u16 flat serialization in C `NmvContext` struct-layout order (the
/// order `c_parity_mv.rs::default_nmv_context_matches_c` proved against the C
/// byte extraction).
fn flatten_nmv(ctx: &NmvContext) -> Vec<u16> {
    let mut flat: Vec<u16> = Vec::with_capacity(cref::NMV_FLAT_LEN);
    flat.extend_from_slice(&ctx.joints_cdf);
    for comp in &ctx.comps {
        flat.extend_from_slice(&comp.classes_cdf);
        for fp in &comp.class0_fp_cdf {
            flat.extend_from_slice(fp);
        }
        flat.extend_from_slice(&comp.fp_cdf);
        flat.extend_from_slice(&comp.sign_cdf);
        flat.extend_from_slice(&comp.class0_hp_cdf);
        flat.extend_from_slice(&comp.hp_cdf);
        flat.extend_from_slice(&comp.class0_cdf);
        for b in &comp.bits_cdf {
            flat.extend_from_slice(b);
        }
    }
    assert_eq!(flat.len(), cref::NMV_FLAT_LEN);
    flat
}

/// Names for every slot of the flat layout, so a CDF-state mismatch reports
/// WHICH context diverged rather than an index.
fn nmv_field_names() -> Vec<String> {
    let mut n = Vec::with_capacity(cref::NMV_FLAT_LEN);
    for i in 0..MV_JOINTS + 1 {
        n.push(format!("joints_cdf[{i}]"));
    }
    for c in 0..2 {
        for i in 0..MV_CLASSES + 1 {
            n.push(format!("comps[{c}].classes_cdf[{i}]"));
        }
        for d in 0..CLASS0_SIZE {
            for i in 0..MV_FP_SIZE + 1 {
                n.push(format!("comps[{c}].class0_fp_cdf[{d}][{i}]"));
            }
        }
        for i in 0..MV_FP_SIZE + 1 {
            n.push(format!("comps[{c}].fp_cdf[{i}]"));
        }
        for i in 0..3 {
            n.push(format!("comps[{c}].sign_cdf[{i}]"));
        }
        for i in 0..3 {
            n.push(format!("comps[{c}].class0_hp_cdf[{i}]"));
        }
        for i in 0..3 {
            n.push(format!("comps[{c}].hp_cdf[{i}]"));
        }
        for i in 0..CLASS0_SIZE + 1 {
            n.push(format!("comps[{c}].class0_cdf[{i}]"));
        }
        for b in 0..MV_OFFSET_BITS {
            for i in 0..3 {
                n.push(format!("comps[{c}].bits_cdf[{b}][{i}]"));
            }
        }
    }
    assert_eq!(n.len(), cref::NMV_FLAT_LEN);
    n
}

fn assert_nmv_eq(rust: &NmvContext, c_flat: &[u16], what: &str) {
    let r = flatten_nmv(rust);
    if r == c_flat {
        return;
    }
    let names = nmv_field_names();
    let first = r
        .iter()
        .zip(c_flat)
        .position(|(a, b)| a != b)
        .expect("lengths equal but vectors differ");
    panic!(
        "{what}: adapted NmvContext diverges at {} (Rust {} vs C {}); \
         {} of {} slots differ",
        names[first],
        r[first],
        c_flat[first],
        r.iter().zip(c_flat).filter(|(a, b)| a != b).count(),
        r.len()
    );
}

const ALL_MODES: [PredictionMode; 25] = [
    PredictionMode::DcPred,
    PredictionMode::VPred,
    PredictionMode::HPred,
    PredictionMode::D45Pred,
    PredictionMode::D135Pred,
    PredictionMode::D113Pred,
    PredictionMode::D157Pred,
    PredictionMode::D203Pred,
    PredictionMode::D67Pred,
    PredictionMode::SmoothPred,
    PredictionMode::SmoothVPred,
    PredictionMode::SmoothHPred,
    PredictionMode::PaethPred,
    PredictionMode::NearestMv,
    PredictionMode::NearMv,
    PredictionMode::GlobalMv,
    PredictionMode::NewMv,
    PredictionMode::NearestNearestMv,
    PredictionMode::NearNearMv,
    PredictionMode::NearestNewMv,
    PredictionMode::NewNearestMv,
    PredictionMode::NearNewMv,
    PredictionMode::NewNearMv,
    PredictionMode::GlobalGlobalMv,
    PredictionMode::NewNewMv,
];

// ---------------------------------------------------------------------------
// §1. Inter-mode predicates (which MVs a block codes)
// ---------------------------------------------------------------------------

/// Every predicate that selects a block's coded MVs, over EVERY prediction
/// mode (intra ones included — C's `is_inter_compound_mode` is a bare range
/// test and is called on raw modes).
#[test]
fn c_parity_inter_mode_predicates() {
    for m in ALL_MODES {
        let raw = m as u8;
        assert_eq!(
            imc::have_newmv_in_inter_mode(m),
            cref::have_newmv_in_inter_mode(raw),
            "have_newmv_in_inter_mode({m:?})"
        );
        assert_eq!(
            imc::have_nearmv_in_inter_mode(m),
            cref::have_nearmv_in_inter_mode(raw),
            "have_nearmv_in_inter_mode({m:?})"
        );
        assert_eq!(
            imc::is_inter_compound_mode(m),
            cref::is_inter_compound_mode(raw),
            "is_inter_compound_mode({m:?})"
        );
        assert_eq!(
            imc::is_inter_singleref_mode(m),
            cref::is_inter_singleref_mode(raw),
            "is_inter_singleref_mode({m:?})"
        );
    }
}

/// C `svt_av1_get_mv_joint` (rd_cost.c:47, EXPORTED) — the joint classifier
/// the RATE side and `av1_update_mv_stats` use, over an already-differenced
/// MV. The port's writer-side twin lives inside `encode_mv_diff`, so this
/// pins the classifier that `update_mv_stats` inlines.
#[test]
fn c_parity_get_mv_joint() {
    let mut rng = Rng(0x0C3_0001);
    let mut cases: Vec<(i16, i16)> = vec![(0, 0), (0, 1), (1, 0), (1, 1), (-1, 0), (0, -1)];
    for _ in 0..2000 {
        // Deliberately biased toward zero so all four joints appear often.
        let z = |rng: &mut Rng| {
            if rng.below(3) == 0 {
                0i16
            } else {
                rng.range_i32(-4096, 4096) as i16
            }
        };
        cases.push((z(&mut rng), z(&mut rng)));
    }
    let mut seen = [0usize; 4];
    for &(x, y) in &cases {
        let c = cref::get_mv_joint((x, y));
        // The port's classifier, as inlined by update_mv_stats.
        let r = if y == 0 {
            if x == 0 { 0 } else { 1 }
        } else if x == 0 {
            2
        } else {
            3
        };
        assert_eq!(r, c, "get_mv_joint(({x},{y}))");
        seen[c as usize] += 1;
    }
    assert!(
        seen.iter().all(|&n| n > 0),
        "corpus never produced every joint type: {seen:?}"
    );
}

// ---------------------------------------------------------------------------
// §2. The writer: bytes AND adapted CDF state vs the real svt_av1_encode_mv
// ---------------------------------------------------------------------------

/// Build a corpus of (mv, ref_mv) pairs that reaches every MV class, both
/// signs, every joint type, and every fractional/hp bit — with NONZERO
/// reference MVs, which the existing `c_parity_mv.rs` suite never uses.
fn mv_corpus(rng: &mut Rng, n: usize) -> (Vec<(i16, i16)>, Vec<(i16, i16)>) {
    let mut mvs: Vec<(i16, i16)> = Vec::new();
    let mut refs: Vec<(i16, i16)> = Vec::new();
    // Deterministic edge cases first.
    let fixed: &[((i16, i16), (i16, i16))] = &[
        ((0, 0), (0, 0)),           // MV_JOINT_ZERO
        ((7, 7), (7, 7)),           // zero diff from a nonzero ref
        ((8, 7), (7, 7)),           // HNZVZ
        ((7, 8), (7, 7)),           // HZVNZ
        ((9, 9), (7, 7)),           // HNZVNZ
        ((-1, -1), (0, 0)),         // negative, class 0
        ((1, 1), (0, 0)),           // positive, class 0
        ((2048, -2048), (-3, 5)),   // high class, both signs
        ((-8191, 8191), (100, -7)), // near the MV_IN_USE range
        ((8191, -8191), (-100, 7)),
    ];
    for &(m, r) in fixed {
        mvs.push(m);
        refs.push(r);
    }
    while mvs.len() < n {
        // Reference MVs from a plausible predictor range; MVs from a wide
        // range so class 0..10 all appear.
        let r = (
            rng.range_i32(-512, 512) as i16,
            rng.range_i32(-512, 512) as i16,
        );
        let spread = [7i32, 63, 511, 4095, 8191][(rng.below(5)) as usize];
        let m = (
            (i32::from(r.0) + rng.range_i32(-spread, spread)).clamp(-8191, 8191) as i16,
            (i32::from(r.1) + rng.range_i32(-spread, spread)).clamp(-8191, 8191) as i16,
        );
        mvs.push(m);
        refs.push(r);
    }
    (mvs, refs)
}

/// Assert the corpus actually reaches the classes/joints the test claims to
/// cover — a silent corpus is indistinguishable from a passing test (see
/// `docs/WORKING-ON-THIS.md` §5).
fn assert_corpus_reaches_every_class(mvs: &[(i16, i16)], refs: &[(i16, i16)]) {
    let mut classes = [0usize; MV_CLASSES];
    let mut joints = [0usize; 4];
    for (&m, &r) in mvs.iter().zip(refs) {
        let dx = i32::from(m.0) - i32::from(r.0);
        let dy = i32::from(m.1) - i32::from(r.1);
        joints[if dy == 0 {
            usize::from(dx != 0)
        } else if dx == 0 {
            2
        } else {
            3
        }] += 1;
        for d in [dx, dy] {
            if d != 0 {
                let (c, _) =
                    svtav1_encoder::entropy::mv_coding::get_mv_class(d.unsigned_abs() as i32 - 1);
                classes[c as usize] += 1;
            }
        }
    }
    assert!(
        joints.iter().all(|&n| n > 0),
        "corpus misses a joint type: {joints:?}"
    );
    assert!(
        classes.iter().all(|&n| n > 0),
        "corpus misses an MV class: {classes:?}"
    );
}

/// The writer, end to end, against the REAL exported `svt_av1_encode_mv`:
/// byte-for-byte output AND the final adapted `NmvContext`, over randomized
/// seed contexts, nonzero reference MVs, and every
/// (`allow_high_precision_mv`, `force_integer_mv`, `allow_update_cdf`)
/// combination.
#[test]
fn c_parity_encode_mv_bytes_and_adapted_cdfs() {
    let mut rng = Rng(0x0C3_1001);
    for iter in 0..4 {
        let seed_ctx = if iter == 0 {
            NmvContext::default()
        } else {
            random_nmv_context(&mut rng)
        };
        let seed_flat = flatten_nmv(&seed_ctx);
        let (mvs, refs) = mv_corpus(&mut rng, 220);
        assert_corpus_reaches_every_class(&mvs, &refs);

        for allow_hp in [false, true] {
            for force_int in [false, true] {
                for allow_update in [false, true] {
                    let c = cref::encode_mv_real_seq(
                        &mvs,
                        &refs,
                        allow_hp,
                        force_int,
                        allow_update,
                        Some(&seed_flat),
                    );

                    let precision = imc::mv_precision(allow_hp, force_int);
                    let mut ctx = seed_ctx.clone();
                    let mut w = AomWriter::new(1 << 17);
                    w.allow_update_cdf = allow_update;
                    for (&m, &r) in mvs.iter().zip(&refs) {
                        imc::encode_mv(
                            &mut w,
                            &mut ctx,
                            Mv { x: m.0, y: m.1 },
                            Mv { x: r.0, y: r.1 },
                            precision,
                        );
                    }
                    let rust_bytes = w.done().to_vec();

                    let label = format!(
                        "iter={iter} hp={allow_hp} force_int={force_int} \
                         update={allow_update}"
                    );
                    assert_eq!(
                        rust_bytes.len(),
                        c.bytes.len(),
                        "{label}: byte COUNT diverges (Rust {} vs C {})",
                        rust_bytes.len(),
                        c.bytes.len()
                    );
                    assert_eq!(rust_bytes, c.bytes, "{label}: coded bytes diverge");
                    assert_nmv_eq(&ctx, &c.nmvc, &label);

                    // Control: with adaptation off the context must be
                    // untouched on BOTH sides — otherwise "CDF state matches"
                    // could be vacuously true.
                    if !allow_update {
                        assert_eq!(c.nmvc, seed_flat, "{label}: C adapted with update off");
                        assert_eq!(
                            flatten_nmv(&ctx),
                            seed_flat,
                            "{label}: port adapted with update off"
                        );
                    }
                }
            }
        }
    }
}

/// The adaptation must actually happen — a test that compares two unchanged
/// contexts proves nothing. Pin that the default context MOVES over the
/// corpus, and that `force_integer_mv` leaves the fractional/hp CDFs alone
/// while quarter/eighth-pel precision moves them.
#[test]
fn adaptation_is_observable_and_precision_gated() {
    let mut rng = Rng(0x0C3_1002);
    let seed = NmvContext::default();
    let seed_flat = flatten_nmv(&seed);
    let (mvs, refs) = mv_corpus(&mut rng, 200);

    let mut moved = Vec::new();
    for (allow_hp, force_int) in [(false, false), (true, false), (true, true)] {
        let c = cref::encode_mv_real_seq(&mvs, &refs, allow_hp, force_int, true, Some(&seed_flat));
        assert_ne!(
            c.nmvc, seed_flat,
            "C left the context unchanged at hp={allow_hp} force_int={force_int}"
        );
        moved.push(c.nmvc.clone());
    }
    // Distinct precisions must produce DISTINCT adapted states, else the
    // precision argument is not reaching the symbol writer at all.
    assert_ne!(
        moved[0], moved[1],
        "LOW and HIGH precision adapt identically"
    );
    assert_ne!(
        moved[1], moved[2],
        "HIGH precision and force_integer_mv adapt identically"
    );

    // Under force_integer_mv no fractional or hp symbol is written, so those
    // CDFs must be byte-identical to the seed while classes_cdf has moved.
    let names = nmv_field_names();
    let none_flat = &moved[2];
    let mut frac_slots = 0;
    for (i, name) in names.iter().enumerate() {
        if name.contains("fp_cdf") || name.contains("hp_cdf") {
            assert_eq!(
                none_flat[i], seed_flat[i],
                "force_integer_mv moved {name}, which it never codes"
            );
            frac_slots += 1;
        }
    }
    assert!(frac_slots > 0, "field-name probe matched nothing");
    let class_idx = names
        .iter()
        .position(|n| n == "comps[0].classes_cdf[0]")
        .unwrap();
    assert_ne!(
        none_flat[class_idx], seed_flat[class_idx],
        "force_integer_mv did not adapt classes_cdf — the probe is inert"
    );
}

/// C's own `entropy_coding.c:5216-5244` branch, transcribed INDEPENDENTLY of
/// `imc::mv_code_plan` so the dispatch test below is not comparing the port
/// against itself. Returns the reference indices C writes, in C's order.
///
/// ```text
/// if (inter_mode == NEWMV || inter_mode == NEW_NEWMV)
///     for (ref = 0; ref < 1 + is_compound; ++ref)   // encode mv[ref]
/// else if (inter_mode == NEAREST_NEWMV || inter_mode == NEAR_NEWMV)
///     encode mv[1]
/// else if (inter_mode == NEW_NEARESTMV || inter_mode == NEW_NEARMV)
///     encode mv[0]
/// ```
fn c_source_refs(m: PredictionMode) -> Vec<usize> {
    use PredictionMode as M;
    if m == M::NewMv || m == M::NewNewMv {
        // `is_compound` is C's is_inter_compound_mode — driven here through
        // the real exported predicate, not a local guess.
        let n = 1 + usize::from(cref::is_inter_compound_mode(m as u8));
        (0..n).collect()
    } else if m == M::NearestNewMv || m == M::NearNewMv {
        vec![1]
    } else if m == M::NewNearestMv || m == M::NewNearMv {
        vec![0]
    } else {
        Vec::new()
    }
}

/// The per-inter-mode writer dispatch (`write_inter_block_mvs`,
/// entropy_coding.c:5216-5244): for each mode, the bytes it emits must equal
/// the bytes the real C writer emits for exactly the MVs C's own branch
/// selects, in C's order.
///
/// The MODE→refs mapping on the oracle side comes from [`c_source_refs`], a
/// transcription of the C branch that does NOT call the port (tier 4 for the
/// mapping; every emitted symbol underneath it is tier 1 through the real
/// `svt_av1_encode_mv`). It is cross-checked against the exported
/// `svt_aom_have_newmv_in_inter_mode` / `is_inter_compound_mode` in
/// `c_parity_mode_plan_agrees_with_c_have_newmv`.
#[test]
fn c_parity_write_inter_block_mvs_dispatch() {
    let mut rng = Rng(0x0C3_1003);
    let seed = random_nmv_context(&mut rng);
    let seed_flat = flatten_nmv(&seed);
    let precision = MvSubpelPrecision::High;

    let mut coded_modes = 0usize;
    for m in ALL_MODES.into_iter().filter(|m| (*m as u8) >= 13) {
        // Distinct MVs per ref slot so picking the WRONG slot changes bytes.
        let mvs = [Mv { x: 33, y: -17 }, Mv { x: -9, y: 512 }];
        let pred = [Mv { x: 1, y: -1 }, Mv { x: -2, y: 8 }];

        let want_refs = c_source_refs(m);
        let selected: Vec<(i16, i16)> = want_refs.iter().map(|&r| (mvs[r].x, mvs[r].y)).collect();
        let selected_refs: Vec<(i16, i16)> =
            want_refs.iter().map(|&r| (pred[r].x, pred[r].y)).collect();
        let c = cref::encode_mv_real_seq(
            &selected,
            &selected_refs,
            true,
            false,
            true,
            Some(&seed_flat),
        );

        let mut ctx = seed.clone();
        let mut w = AomWriter::new(1 << 12);
        w.allow_update_cdf = true;
        let got = imc::write_inter_block_mvs(&mut w, &mut ctx, m, &mvs, &pred, precision);
        assert_eq!(got.refs(), want_refs.as_slice(), "{m:?}: coded-ref set");
        let bytes = w.done().to_vec();

        assert_eq!(bytes, c.bytes, "{m:?}: coded MV bytes diverge");
        assert_nmv_eq(&ctx, &c.nmvc, &format!("{m:?}"));
        coded_modes += usize::from(!want_refs.is_empty());
    }
    // Anti-vacuity: the sweep must actually code MVs for six modes, else
    // "bytes match" is comparing two empty streams.
    assert_eq!(coded_modes, 6, "the NEWMV-family sweep did not fire");
}

/// The mode→plan mapping must select an MV for exactly the modes C's
/// `svt_aom_have_newmv_in_inter_mode` (EXPORTED) identifies, and the compound
/// arm must code two MVs for exactly the compound NEW_NEWMV case.
#[test]
fn c_parity_mode_plan_agrees_with_c_have_newmv() {
    for m in ALL_MODES.into_iter().filter(|m| (*m as u8) >= 13) {
        let plan = imc::mv_code_plan(m);
        assert_eq!(
            plan != imc::MvCodePlan::None,
            cref::have_newmv_in_inter_mode(m as u8),
            "{m:?}: plan {plan:?} disagrees with C have_newmv_in_inter_mode"
        );
        // C's loop bound is `1 + is_compound` and only fires on the
        // NEWMV/NEW_NEWMV branch, so two MVs iff compound-and-new-new.
        let two = plan.refs().len() == 2;
        assert_eq!(
            two,
            m == PredictionMode::NewNewMv,
            "{m:?}: coded-MV count disagrees with C's `1 + is_compound` loop"
        );
        // The single-MV NEW_NEAR* / NEAR_NEW* split: C reads predmv[0] for
        // NEW_NEARESTMV/NEW_NEARMV and predmv[1] for NEAREST_NEWMV/NEAR_NEWMV.
        if matches!(m, PredictionMode::NearestNewMv | PredictionMode::NearNewMv) {
            assert_eq!(plan, imc::MvCodePlan::Ref1, "{m:?}");
        }
        if matches!(m, PredictionMode::NewNearestMv | PredictionMode::NewNearMv) {
            assert_eq!(plan, imc::MvCodePlan::Ref0, "{m:?}");
        }
    }
}

// ---------------------------------------------------------------------------
// §3. CDF adaptation (av1_update_mv_stats)
// ---------------------------------------------------------------------------

/// `update_mv_stats` / `update_mv_component_stats` (md_rate_estimation.c:650-705)
/// perform the SAME `update_cdf` sequence, over the same CDFs, in the same
/// order, that `aom_write_symbol` performs while WRITING the same MV. So the
/// oracle is the adapted context the real exported `svt_av1_encode_mv` leaves
/// behind — a tier-1 differential.
///
/// (`av1_update_mv_stats` itself is `static` in C with no exported symbol.
/// The claim being gated is exactly the equivalence above: if the port's
/// stats replay diverged from the writer's adaptation in ANY field, this
/// fails and names the field.)
#[test]
fn c_parity_update_mv_stats_matches_writer_adaptation() {
    let mut rng = Rng(0x0C3_2001);
    for iter in 0..4 {
        let seed = if iter == 0 {
            NmvContext::default()
        } else {
            random_nmv_context(&mut rng)
        };
        let seed_flat = flatten_nmv(&seed);
        let (mvs, refs) = mv_corpus(&mut rng, 200);
        assert_corpus_reaches_every_class(&mvs, &refs);

        for allow_hp in [false, true] {
            for force_int in [false, true] {
                let c = cref::encode_mv_real_seq(
                    &mvs,
                    &refs,
                    allow_hp,
                    force_int,
                    true,
                    Some(&seed_flat),
                );
                let precision = imc::mv_precision(allow_hp, force_int);
                let mut ctx = seed.clone();
                for (&m, &r) in mvs.iter().zip(&refs) {
                    imc::update_mv_stats(
                        &mut ctx,
                        Mv { x: m.0, y: m.1 },
                        Mv { x: r.0, y: r.1 },
                        precision,
                    );
                }
                assert_nmv_eq(
                    &ctx,
                    &c.nmvc,
                    &format!("update_mv_stats iter={iter} hp={allow_hp} force_int={force_int}"),
                );
            }
        }
    }
}

/// The per-block stats dispatch must replay exactly the MVs the writer
/// dispatch codes — MD's shadow context would otherwise drift from the
/// bitstream's.
#[test]
fn update_inter_block_mv_stats_tracks_the_writer() {
    let mut rng = Rng(0x0C3_2002);
    let seed = random_nmv_context(&mut rng);
    let mvs = [Mv { x: 40, y: -24 }, Mv { x: -12, y: 300 }];
    let pred = [Mv { x: 3, y: -1 }, Mv { x: -5, y: 11 }];
    for m in ALL_MODES.into_iter().filter(|m| (*m as u8) >= 13) {
        let precision = MvSubpelPrecision::High;

        let mut written = seed.clone();
        let mut w = AomWriter::new(1 << 12);
        w.allow_update_cdf = true;
        imc::write_inter_block_mvs(&mut w, &mut written, m, &mvs, &pred, precision);

        let mut replayed = seed.clone();
        let plan = imc::update_inter_block_mv_stats(&mut replayed, m, &mvs, &pred, precision);
        assert_eq!(plan, imc::mv_code_plan(m));

        assert_eq!(
            flatten_nmv(&written),
            flatten_nmv(&replayed),
            "{m:?}: stats replay diverges from the writer's adaptation"
        );
    }
}

/// `reset_nmv_counter` (cabac_context_model.c:1956), driven through the
/// EXPORTED `svt_av1_reset_cdf_symbol_counters`.
#[test]
fn c_parity_reset_nmv_counter() {
    let mut rng = Rng(0x0C3_2003);
    for iter in 0..6 {
        let mut ctx = if iter == 0 {
            NmvContext::default()
        } else {
            random_nmv_context(&mut rng)
        };
        let flat = flatten_nmv(&ctx);
        let c = cref::reset_nmv_counter(&flat);
        // Anti-vacuity: the randomized contexts carry nonzero counters, so
        // the reset must actually change something.
        if iter > 0 {
            assert_ne!(c, flat, "iter {iter}: C reset changed nothing");
        }
        imc::reset_nmv_counter(&mut ctx);
        assert_nmv_eq(&ctx, &c, &format!("reset_nmv_counter iter={iter}"));
    }
}

/// `avg_nmv` (enc_dec_process.c:2567) has no exported symbol — this is a
/// **tier-4** check (hand-derived against the C source), and what it actually
/// proves is FIELD COVERAGE: C's `AVERAGE_CDF` touches every element of every
/// `NmvContext` array, so averaging the two flat serializations element-wise
/// is the whole function. A missed field in the port shows up immediately.
///
/// The per-element arithmetic itself is `avg_cdf_entries`, already gated
/// against C's `avg_cdf_symbol` in `entropy::cdf`.
#[test]
fn avg_nmv_covers_every_field() {
    let mut rng = Rng(0x0C3_2004);
    for iter in 0..4 {
        let mut left = random_nmv_context(&mut rng);
        let tr = random_nmv_context(&mut rng);
        let (lf, tf) = (flatten_nmv(&left), flatten_nmv(&tr));
        let (wt_left, wt_tr) = (3i32, 1i32); // AVG_CDF_WEIGHT_LEFT / _TOP
        let want: Vec<u16> = lf
            .iter()
            .zip(&tf)
            .map(|(&l, &t)| {
                ((i32::from(l) * wt_left + i32::from(t) * wt_tr + (wt_left + wt_tr) / 2)
                    / (wt_left + wt_tr)) as u16
            })
            .collect();
        assert_ne!(want, lf, "iter {iter}: averaging changed nothing");
        imc::avg_nmv(&mut left, &tr, wt_left, wt_tr);
        assert_nmv_eq(&left, &want, &format!("avg_nmv iter={iter}"));
    }
}

// ---------------------------------------------------------------------------
// §4. MV rate — svt_aom_estimate_mv_rate and the per-mode dispatch
// ---------------------------------------------------------------------------

const DV_SENTINEL: i32 = -0x5EAD;

/// The full `svt_aom_estimate_mv_rate` (md_rate_estimation.c:458-488):
/// every joint cost and every one of the 2 x 32767 component costs, for both
/// hp arms, with and without the intrabc dv arm, over randomized contexts.
#[test]
fn c_parity_estimate_mv_rate_tables() {
    let mut rng = Rng(0x0C3_3001);
    for iter in 0..3 {
        let (nmvc, ndvc) = if iter == 0 {
            (NmvContext::default(), NmvContext::default())
        } else {
            (random_nmv_context(&mut rng), random_nmv_context(&mut rng))
        };
        let (nf, df) = (flatten_nmv(&nmvc), flatten_nmv(&ndvc));
        for hp in [false, true] {
            for allow_intrabc in [false, true] {
                let c = cref::estimate_mv_rate(
                    false,
                    allow_intrabc,
                    hp,
                    Some(&nf),
                    Some(&df),
                    DV_SENTINEL,
                );
                let rs = imc::estimate_mv_rate(&nmvc, &ndvc, hp, allow_intrabc, false);
                let label = format!("iter={iter} hp={hp} ibc={allow_intrabc}");

                let nmv = rs.nmv.tables().expect("non-approx must build nmv tables");
                assert_eq!(
                    nmv.joint_cost.as_slice(),
                    &c.nmv_joint,
                    "{label}: nmv joint"
                );
                for comp in 0..2 {
                    let c_col = &c.nmv_costs[comp * cref::MV_VALS..(comp + 1) * cref::MV_VALS];
                    for v in -intrabc::MV_MAX..=intrabc::MV_MAX {
                        let idx = (intrabc::MV_MAX + v) as usize;
                        assert_eq!(
                            nmv.comp_cost[comp].cost(v),
                            c_col[idx],
                            "{label}: nmv cost comp={comp} v={v}"
                        );
                    }
                }

                match (&rs.dv, allow_intrabc) {
                    (Some(dv), true) => {
                        assert_eq!(dv.joint_cost.as_slice(), &c.dv_joint, "{label}: dv joint");
                        for comp in 0..2 {
                            let c_col =
                                &c.dv_costs[comp * cref::MV_VALS..(comp + 1) * cref::MV_VALS];
                            for v in -intrabc::MV_MAX..=intrabc::MV_MAX {
                                let idx = (intrabc::MV_MAX + v) as usize;
                                assert_eq!(
                                    dv.comp_cost[comp].cost(v),
                                    c_col[idx],
                                    "{label}: dv cost comp={comp} v={v}"
                                );
                            }
                        }
                    }
                    (None, false) => {
                        // C leaves the dv arrays UNTOUCHED (the sentinel the
                        // shim seeded survives) — not zeroed.
                        assert!(
                            c.dv_joint.iter().all(|&v| v == DV_SENTINEL)
                                && c.dv_costs.iter().all(|&v| v == DV_SENTINEL),
                            "{label}: C filled dv without allow_intrabc"
                        );
                    }
                    (dv, _) => panic!("{label}: dv arm {:?} vs allow_intrabc", dv.is_some()),
                }
            }
        }
    }
}

/// The `approx_inter_rate` early return (md_rate_estimation.c:459-465): nmv
/// tables ZEROED (so every MV costs 0 through them), dv arm SKIPPED even when
/// `allow_intrabc` — the ordering hazard.
#[test]
fn c_parity_estimate_mv_rate_approx_arm() {
    let mut rng = Rng(0x0C3_3002);
    let ctx = random_nmv_context(&mut rng);
    let flat = flatten_nmv(&ctx);
    for allow_intrabc in [false, true] {
        for hp in [false, true] {
            let c = cref::estimate_mv_rate(
                true,
                allow_intrabc,
                hp,
                Some(&flat),
                Some(&flat),
                DV_SENTINEL,
            );
            assert!(c.nmv_joint.iter().all(|&v| v == 0));
            assert!(c.nmv_costs.iter().all(|&v| v == 0));
            assert!(
                c.dv_joint.iter().all(|&v| v == DV_SENTINEL)
                    && c.dv_costs.iter().all(|&v| v == DV_SENTINEL),
                "approx must return BEFORE the dv arm (ibc={allow_intrabc})"
            );

            let rs = imc::estimate_mv_rate(&ctx, &ctx, hp, allow_intrabc, true);
            assert!(rs.dv.is_none(), "port filled dv on the approx arm");
            assert!(
                rs.nmv.tables().is_none(),
                "port built nmv on the approx arm"
            );
            // The observable consequence of the zero fill: every rate is 0.
            for &(mv, rf) in &[
                ((0i16, 0i16), (0i16, 0i16)),
                ((1024, -777), (-3, 9)),
                ((-8191, 8191), (0, 0)),
            ] {
                let (mv, rf) = (Mv { x: mv.0, y: mv.1 }, Mv { x: rf.0, y: rf.1 });
                assert_eq!(imc::mv_bit_cost(mv, rf, &rs.nmv, imc::MV_COST_WEIGHT), 0);
                let c_zero_joint = [0i32; 4];
                let c_zero = vec![0i32; cref::MV_VALS];
                assert_eq!(
                    cref::mv_bit_cost(
                        (mv.x, mv.y),
                        (rf.x, rf.y),
                        &c_zero_joint,
                        &c_zero,
                        &c_zero,
                        imc::MV_COST_WEIGHT
                    ),
                    0,
                    "C's zeroed tables must also cost 0"
                );
            }
        }
    }
}

/// `svt_av1_mv_bit_cost` (rd_cost.c:70, EXPORTED) through the nmv tables the
/// real `svt_aom_estimate_mv_rate` built, at the inter weight
/// `MV_COST_WEIGHT = 108`, plus the `_light` twin.
#[test]
fn c_parity_mv_bit_cost_over_nmv_tables() {
    let mut rng = Rng(0x0C3_3003);
    for iter in 0..3 {
        let ctx = if iter == 0 {
            NmvContext::default()
        } else {
            random_nmv_context(&mut rng)
        };
        let flat = flatten_nmv(&ctx);
        for hp in [false, true] {
            let c = cref::estimate_mv_rate(false, false, hp, Some(&flat), None, DV_SENTINEL);
            let (c0, c1) = c.nmv_costs.split_at(cref::MV_VALS);
            let rs = imc::estimate_mv_rate(&ctx, &ctx, hp, false, false);

            let (mvs, refs) = mv_corpus(&mut rng, 300);
            let mut nonzero = 0usize;
            for (&m, &r) in mvs.iter().zip(&refs) {
                let (mv, rf) = (Mv { x: m.0, y: m.1 }, Mv { x: r.0, y: r.1 });
                let want = cref::mv_bit_cost(m, r, &c.nmv_joint, c0, c1, imc::MV_COST_WEIGHT);
                let got = imc::mv_bit_cost(mv, rf, &rs.nmv, imc::MV_COST_WEIGHT);
                assert_eq!(got, want, "iter={iter} hp={hp} mv={m:?} ref={r:?}");
                nonzero += usize::from(got != 0);
                assert_eq!(
                    imc::mv_bit_cost_light(mv, rf),
                    cref::mv_bit_cost_light(m, r),
                    "light: mv={m:?} ref={r:?}"
                );
            }
            assert!(
                nonzero > mvs.len() / 2,
                "iter={iter} hp={hp}: costs were mostly zero — tables look empty"
            );
        }
    }
}

/// The per-inter-mode rate dispatch (`svt_aom_inter_fast_cost`'s `mv_rate`
/// term, rd_cost.c:1088-1128): the total must equal the sum of the real C
/// `svt_av1_mv_bit_cost` over exactly the MVs C's own branch selects.
#[test]
fn c_parity_inter_mv_rate_dispatch() {
    let mut rng = Rng(0x0C3_3004);
    let ctx = random_nmv_context(&mut rng);
    let flat = flatten_nmv(&ctx);
    let c = cref::estimate_mv_rate(false, false, true, Some(&flat), None, DV_SENTINEL);
    let (c0, c1) = c.nmv_costs.split_at(cref::MV_VALS);
    let rs = imc::estimate_mv_rate(&ctx, &ctx, true, false, false);

    for _ in 0..64 {
        let mvs = [
            Mv {
                x: rng.range_i32(-2048, 2048) as i16,
                y: rng.range_i32(-2048, 2048) as i16,
            },
            Mv {
                x: rng.range_i32(-2048, 2048) as i16,
                y: rng.range_i32(-2048, 2048) as i16,
            },
        ];
        let pred = [
            Mv {
                x: rng.range_i32(-256, 256) as i16,
                y: rng.range_i32(-256, 256) as i16,
            },
            Mv {
                x: rng.range_i32(-256, 256) as i16,
                y: rng.range_i32(-256, 256) as i16,
            },
        ];
        for m in ALL_MODES.into_iter().filter(|m| (*m as u8) >= 13) {
            // C's own branch, transcribed from rd_cost.c:1088-1128.
            let want: i32 = if !cref::have_newmv_in_inter_mode(m as u8) {
                0
            } else if cref::is_inter_compound_mode(m as u8) {
                if m == PredictionMode::NewNewMv {
                    (0..2)
                        .map(|r| {
                            cref::mv_bit_cost(
                                (mvs[r].x, mvs[r].y),
                                (pred[r].x, pred[r].y),
                                &c.nmv_joint,
                                c0,
                                c1,
                                imc::MV_COST_WEIGHT,
                            )
                        })
                        .sum()
                } else if matches!(m, PredictionMode::NearestNewMv | PredictionMode::NearNewMv) {
                    cref::mv_bit_cost(
                        (mvs[1].x, mvs[1].y),
                        (pred[1].x, pred[1].y),
                        &c.nmv_joint,
                        c0,
                        c1,
                        imc::MV_COST_WEIGHT,
                    )
                } else {
                    cref::mv_bit_cost(
                        (mvs[0].x, mvs[0].y),
                        (pred[0].x, pred[0].y),
                        &c.nmv_joint,
                        c0,
                        c1,
                        imc::MV_COST_WEIGHT,
                    )
                }
            } else {
                // single ref: unipred MV in idx 0
                cref::mv_bit_cost(
                    (mvs[0].x, mvs[0].y),
                    (pred[0].x, pred[0].y),
                    &c.nmv_joint,
                    c0,
                    c1,
                    imc::MV_COST_WEIGHT,
                )
            };
            let got = imc::inter_mv_rate(m, &mvs, &pred, &rs.nmv, imc::MV_COST_WEIGHT);
            assert_eq!(got, want, "{m:?}: inter mv_rate diverges");
        }
    }
}

/// Under `force_integer_mv` the WRITER codes at `MV_SUBPEL_NONE` but the RATE
/// tables are still built at `MV_SUBPEL_LOW_PRECISION` — C's
/// `svt_aom_estimate_mv_rate` passes `allow_high_precision_mv` straight in and
/// never consults `force_integer_mv` (md_rate_estimation.c:474-478). Pin that
/// asymmetry so nobody "fixes" it: the port's tables at
/// `force_integer_mv = true, allow_hp = false` must equal C's, and must NOT
/// equal a `MvSubpelPrecision::None` build.
#[test]
fn c_parity_rate_tables_ignore_force_integer_mv() {
    let mut rng = Rng(0x0C3_3005);
    let ctx = random_nmv_context(&mut rng);
    let flat = flatten_nmv(&ctx);
    let c = cref::estimate_mv_rate(false, false, false, Some(&flat), None, DV_SENTINEL);
    let rs = imc::estimate_mv_rate(&ctx, &ctx, false, false, false);
    let nmv = rs.nmv.tables().unwrap();
    let (c0, _c1) = c.nmv_costs.split_at(cref::MV_VALS);
    for v in -intrabc::MV_MAX..=intrabc::MV_MAX {
        let idx = (intrabc::MV_MAX + v) as usize;
        assert_eq!(nmv.comp_cost[0].cost(v), c0[idx], "v={v}");
    }
    // And a NONE-precision build genuinely differs, so the assertion above is
    // not vacuous.
    let none = intrabc::build_nmv_cost_table(&ctx, MvSubpelPrecision::None);
    let differs = (-intrabc::MV_MAX..=intrabc::MV_MAX)
        .any(|v| none.comp_cost[0].cost(v) != nmv.comp_cost[0].cost(v));
    assert!(
        differs,
        "LOW and NONE precision tables are identical — the pin proves nothing"
    );
}

// ---------------------------------------------------------------------------
// §5. The update_mv cadence
// ---------------------------------------------------------------------------

/// `svt_aom_get_update_cdf_level_{default,rtc,allintra}` (enc_mode_config.c:
/// 8510/8524/8534, all EXPORTED) across every preset the port can express
/// (`SpeedConfig::preset` is a `u8`, so M0..M13 plus the saturating tail) and
/// both slice types.
#[test]
fn c_parity_update_cdf_level_derivations() {
    for enc_mode in 0..=13i32 {
        assert_eq!(
            imc::update_cdf_level_allintra(enc_mode),
            cref::update_cdf_level_allintra(enc_mode),
            "allintra M{enc_mode}"
        );
        for is_i in [false, true] {
            assert_eq!(
                imc::update_cdf_level_rtc(enc_mode, is_i),
                cref::update_cdf_level_rtc(enc_mode, is_i),
                "rtc M{enc_mode} islice={is_i}"
            );
            for is_base in [false, true] {
                assert_eq!(
                    imc::update_cdf_level_default(enc_mode, is_i, is_base),
                    cref::update_cdf_level_default(enc_mode, is_i, is_base),
                    "default M{enc_mode} islice={is_i} base={is_base}"
                );
            }
        }
    }
    // Anti-vacuity: the derivations must not be constant across the sweep.
    let levels: Vec<u8> = (0..=13)
        .map(|m| cref::update_cdf_level_default(m, false, false))
        .collect();
    assert!(
        levels.iter().any(|&l| l != levels[0]),
        "update_cdf_level_default is constant — the sweep proves nothing"
    );
}

/// `set_cdf_controls`' `update_mv` arm (enc_mode_config.c:8468-8498) is
/// `static` in C — **tier 4**, transcribed from source. Its consequence is
/// what matters and is worth stating: MV CDFs adapt for MD only at
/// `update_cdf_level == 1` on a non-I slice, which is why the whole
/// still-image envelope never exercises this path and why every test above
/// needed its own oracle instead of an identity cell.
#[test]
fn update_mv_cadence_is_level1_non_i_slice_only() {
    for level in 0u8..=3 {
        assert!(!imc::cdf_update_mv(level, true), "I-slice level {level}");
        assert_eq!(
            imc::cdf_update_mv(level, false),
            level == 1,
            "B-slice level {level}"
        );
    }
    // Cross-check against the real derivations: an all-intra still at any
    // preset the port can express never reaches update_mv.
    for enc_mode in 0..=13i32 {
        let level = cref::update_cdf_level_allintra(enc_mode);
        assert!(
            !imc::cdf_update_mv(level, true),
            "all-intra M{enc_mode} (level {level}) would adapt MV CDFs"
        );
    }
}

/// `copy_mv_rate` + its cadence (`sb_mv_rate`, enc_dec_process.c:36-56 and
/// :2802-2806 / :2908-2912). The COPY arm must reuse the frame tables
/// verbatim and must NOT rebuild from the SB's adapted context; the REBUILD
/// arm must equal a direct `svt_aom_estimate_mv_rate` over that SB context.
/// Both arms are checked against C's own tables, and the two arms are proved
/// DISTINCT so "reuse" is not vacuously "rebuild".
#[test]
fn c_parity_sb_mv_rate_cadence() {
    let mut rng = Rng(0x0C3_4001);
    let frame_ctx = NmvContext::default();
    let sb_ctx = random_nmv_context(&mut rng);
    let (ff, sf) = (flatten_nmv(&frame_ctx), flatten_nmv(&sb_ctx));
    assert_ne!(ff, sf, "the two contexts must differ for this test to bite");

    for hp in [false, true] {
        let c_frame = cref::estimate_mv_rate(false, false, hp, Some(&ff), None, DV_SENTINEL);
        let c_sb = cref::estimate_mv_rate(false, false, hp, Some(&sf), None, DV_SENTINEL);
        assert_ne!(
            c_frame.nmv_costs, c_sb.nmv_costs,
            "hp={hp}: C built identical tables from different contexts"
        );

        let frame_rate = imc::estimate_mv_rate(&frame_ctx, &frame_ctx, hp, false, false);

        // update_mv OFF: the frame tables, byte for byte.
        let copied = imc::sb_mv_rate(false, &frame_rate, &sb_ctx, &sb_ctx, hp, false, false);
        // update_mv ON: rebuilt from the SB context.
        let rebuilt = imc::sb_mv_rate(true, &frame_rate, &sb_ctx, &sb_ctx, hp, false, false);

        let (cf, cs) = (copied.nmv.tables().unwrap(), rebuilt.nmv.tables().unwrap());
        assert_eq!(
            cf.joint_cost.as_slice(),
            &c_frame.nmv_joint,
            "copy joint hp={hp}"
        );
        assert_eq!(
            cs.joint_cost.as_slice(),
            &c_sb.nmv_joint,
            "rebuild joint hp={hp}"
        );
        for comp in 0..2 {
            let cfc = &c_frame.nmv_costs[comp * cref::MV_VALS..(comp + 1) * cref::MV_VALS];
            let csc = &c_sb.nmv_costs[comp * cref::MV_VALS..(comp + 1) * cref::MV_VALS];
            for v in -intrabc::MV_MAX..=intrabc::MV_MAX {
                let idx = (intrabc::MV_MAX + v) as usize;
                assert_eq!(
                    cf.comp_cost[comp].cost(v),
                    cfc[idx],
                    "copy comp={comp} v={v}"
                );
                assert_eq!(
                    cs.comp_cost[comp].cost(v),
                    csc[idx],
                    "rebuild comp={comp} v={v}"
                );
            }
        }
    }
}

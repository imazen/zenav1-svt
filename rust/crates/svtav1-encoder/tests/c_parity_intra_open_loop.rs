//! Differential parity for the open-loop intra predictor and `is_smooth` vs
//! the real exported C symbols — evidence tier 1
//! (`docs/WORKING-ON-THIS.md` §4).
//!
//! `svt_aom_intra_prediction_open_loop_mb` is the predictor the OIS pass
//! runs, and OIS output feeds TPL and motion estimation — a wrong pixel here
//! is a wrong inter decision, not a wrong intra block.
//!
//! It is also a DISPATCHED entry point in the trap-2 sense: the non-
//! directional modes go through `svt_aom_eb_pred[mode][tx_size]` and
//! `svt_aom_dc_pred[left][top][tx_size]`, two tables that are null until
//! `svt_aom_init_intra_predictors_internal` runs, and whose x86 members are
//! AVX2/AVX-512 kernels. The shim inits them and re-stages every buffer into
//! 64-byte-aligned locals; `rtcd_ready` is asserted first so a null table
//! cannot masquerade as agreement.

use svtav1_cref::pic_operators as cref_po;
use svtav1_encoder::intra_open_loop::{
    Neighbours, intra_prediction_open_loop_mb, is_directional_mode, is_smooth,
};
use svtav1_types::prediction::{PredictionMode, UvPredictionMode};

const EDGE_LEN: usize = cref_po::EDGE_BUF_LEN;
const ORIGIN: usize = cref_po::EDGE_ORIGIN;

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
}

/// `(TxSize, width, height)` for every AV1 transform shape the OIS pass can
/// hand this function. TxSize values are the C enum order (definitions.h).
const TX_SHAPES: &[(i32, usize, usize)] = &[
    (0, 4, 4),
    (1, 8, 8),
    (2, 16, 16),
    (3, 32, 32),
    (4, 64, 64),
    (5, 4, 8),
    (6, 8, 4),
    (7, 8, 16),
    (8, 16, 8),
    (9, 16, 32),
    (10, 32, 16),
    (11, 32, 64),
    (12, 64, 32),
    (13, 4, 16),
    (14, 16, 4),
    (15, 8, 32),
    (16, 32, 8),
    (17, 16, 64),
    (18, 64, 16),
];

/// Every `p_angle` the encoder can hand the directional predictor:
/// `MODE_TO_ANGLE_MAP[mode] + delta * 3`, `delta` in -3..=3.
fn producible_angles() -> Vec<i32> {
    let mut v = Vec::new();
    for base in [90, 180, 45, 135, 113, 157, 203, 67] {
        for step in [-9, -6, -3, 0, 3, 6, 9] {
            v.push(base + step);
        }
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// Every intra mode, paired with the `p_angle` C would pass. Non-directional
/// modes ignore the angle entirely; directional ones use the nominal angle
/// plus the two delta steps the OIS pass can produce.
fn mode_angles() -> Vec<(PredictionMode, i32)> {
    use PredictionMode::*;
    let mut v = vec![
        (DcPred, 0),
        (SmoothPred, 0),
        (SmoothVPred, 0),
        (SmoothHPred, 0),
        (PaethPred, 0),
    ];
    // Nominal angles: V=90, H=180, D45=45, D135=135, D113=113, D157=157,
    // D203=203, D67=67 (mode_to_angle_map, intra_prediction.c).
    for (m, base) in [
        (VPred, 90),
        (HPred, 180),
        (D45Pred, 45),
        (D135Pred, 135),
        (D113Pred, 113),
        (D157Pred, 157),
        (D203Pred, 203),
        (D67Pred, 67),
    ] {
        for step in [-9, -6, -3, 0, 3, 6, 9] {
            v.push((m, base + step));
        }
    }
    v
}

#[test]
fn rtcd_and_predictor_tables_are_bound() {
    assert!(
        cref_po::rtcd_ready(),
        "svt_aom_eb_pred / the RTCD slots are unbound; every comparison in \
         this file would be meaningless"
    );
}

#[test]
fn intra_prediction_open_loop_mb_matches_c() {
    let mut rng = Rng(0xFACE_B00C);
    let mut compared = 0usize;
    for &(tx_size, w, h) in TX_SHAPES {
        // The edged buffers, filled exactly once per shape so the port and C
        // see byte-identical neighbours.
        let mut above = [0u8; EDGE_LEN];
        let mut left = [0u8; EDGE_LEN];
        for v in above.iter_mut() {
            *v = rng.next() as u8;
        }
        for v in left.iter_mut() {
            *v = rng.next() as u8;
        }
        // C's above_row[-1] and left_col[-1] are the SAME corner sample.
        let top_left = above[ORIGIN - 1];
        left[ORIGIN - 1] = top_left;

        for (mode, angle) in mode_angles() {
            for (has_left, has_above) in
                [(false, false), (false, true), (true, false), (true, true)]
            {
                let stride = w + 5;
                let mut mine = vec![0u8; stride * h];
                let mut theirs = vec![0u8; stride * h];

                let n = Neighbours {
                    above: &above[ORIGIN..],
                    left: &left[ORIGIN..],
                    top_left,
                    has_left,
                    has_above,
                };
                intra_prediction_open_loop_mb(mode, angle, n, w, h, &mut mine, stride)
                    .expect("every case here is in range");

                cref_po::intra_prediction_open_loop_mb(
                    angle,
                    mode as u8,
                    u32::from(has_left),
                    u32::from(has_above),
                    tx_size,
                    &above,
                    &left,
                    &mut theirs,
                    stride,
                    w,
                    h,
                );

                assert_eq!(
                    mine, theirs,
                    "open_loop_mb {mode:?} angle {angle} tx {tx_size} {w}x{h} \
                     has_left {has_left} has_above {has_above}"
                );
                compared += 1;
            }
        }
    }
    assert!(compared >= 4000, "coverage collapsed to {compared} cases");
}

/// `svt_aom_dr_predictor` is the directional half of the entry point above,
/// compared on its own so a directional divergence localizes without the
/// mode dispatch in the way.
#[test]
fn dr_predictor_matches_c() {
    let mut rng = Rng(0x0DDB_A11);
    for &(tx_size, w, h) in TX_SHAPES {
        let mut above = [0u8; EDGE_LEN];
        let mut left = [0u8; EDGE_LEN];
        for v in above.iter_mut() {
            *v = rng.next() as u8;
        }
        for v in left.iter_mut() {
            *v = rng.next() as u8;
        }
        let top_left = above[ORIGIN - 1];
        left[ORIGIN - 1] = top_left;

        // ONLY the angles the encoder can produce. `p_angle` is always
        // `MODE_TO_ANGLE_MAP[mode] + delta * 3` with `delta` in -3..=3
        // (intra_edge.rs:1724 and the three leaf_funnel sites), so the
        // reachable set is 8 nominal angles +/- 9 in steps of 3.
        // A naive `(3..270).step_by(3)` sweep is OUT of that domain and
        // lands on angles whose `dr_intra_derivative` entry is 0 — C
        // asserts `dx > 0` there and only survives because Release builds
        // with NDEBUG. Bounding the generator by what the PRODUCER can
        // produce is the rule (WORKING-ON-THIS §5 trap 5).
        for angle in producible_angles() {
            let stride = w;
            let mut mine = vec![0u8; stride * h];
            let mut theirs = vec![0u8; stride * h];
            svtav1_dsp::intra_pred::predict_directional(
                &mut mine,
                stride,
                &above[ORIGIN..],
                &left[ORIGIN..],
                top_left,
                w,
                h,
                angle,
            );
            cref_po::dr_predictor(
                &mut theirs,
                stride,
                tx_size,
                &above,
                &left,
                0,
                0,
                angle,
                w,
                h,
            );
            assert_eq!(
                mine, theirs,
                "dr_predictor angle {angle} tx {tx_size} {w}x{h}"
            );
        }
    }
}

#[test]
fn is_smooth_matches_c_over_every_mode_and_plane() {
    for mode_raw in 0..25u8 {
        for uv_raw in 0..14u8 {
            for plane in 0..3i32 {
                for &(ref_frame_0, is_inter) in &[(0i32, false), (1, true), (7, true)] {
                    let theirs = cref_po::intra_is_smooth_with_ref(
                        i32::from(mode_raw),
                        i32::from(uv_raw),
                        plane,
                        ref_frame_0,
                    );
                    // The port takes the mode enums; only intra modes are
                    // representable for luma, so drive the raw values
                    // through the same classification C uses.
                    let mine = port_is_smooth(mode_raw, uv_raw, plane as usize, is_inter);
                    assert_eq!(
                        mine, theirs,
                        "is_smooth mode {mode_raw} uv {uv_raw} plane {plane} ref {ref_frame_0}"
                    );
                }
            }
        }
    }
}

/// Route raw C enum values through the port's typed `is_smooth`. Values that
/// are not valid `PredictionMode` / `UvPredictionMode` members cannot occur
/// in the encoder, so this maps them to a mode the predicate treats
/// identically (any non-SMOOTH one).
fn port_is_smooth(mode_raw: u8, uv_raw: u8, plane: usize, is_inter: bool) -> bool {
    let mode = MODES[usize::from(mode_raw)];
    let uv = UV_MODES[usize::from(uv_raw)];
    is_smooth(mode, uv, plane, is_inter)
}

const MODES: [PredictionMode; 25] = {
    use PredictionMode::*;
    [
        DcPred,
        VPred,
        HPred,
        D45Pred,
        D135Pred,
        D113Pred,
        D157Pred,
        D203Pred,
        D67Pred,
        SmoothPred,
        SmoothVPred,
        SmoothHPred,
        PaethPred,
        NearestMv,
        NearMv,
        GlobalMv,
        NewMv,
        NearestNearestMv,
        NearNearMv,
        NearestNewMv,
        NewNearestMv,
        NearNewMv,
        NewNearMv,
        GlobalGlobalMv,
        NewNewMv,
    ]
};

const UV_MODES: [UvPredictionMode; 14] = {
    use UvPredictionMode::*;
    [
        UvDcPred,
        UvVPred,
        UvHPred,
        UvD45Pred,
        UvD135Pred,
        UvD113Pred,
        UvD157Pred,
        UvD203Pred,
        UvD67Pred,
        UvSmoothPred,
        UvSmoothVPred,
        UvSmoothHPred,
        UvPaethPred,
        UvCflPred,
    ]
};

#[test]
fn is_directional_mode_matches_the_c_range() {
    for (i, &m) in MODES.iter().enumerate() {
        // C: mode >= V_PRED (1) && mode <= D67_PRED (8).
        assert_eq!(is_directional_mode(m), (1..=8).contains(&i), "mode {i}");
    }
}

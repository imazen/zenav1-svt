use super::*;
use svtav1_types::prediction::PredictionMode as M;

/// Every C `PredictionMode` in enum order, so the sweep below indexes by
/// C's raw value (which is what `block_mi.mode` carries).
const ALL_MODES: [M; M::COUNT] = [
    M::DcPred,
    M::VPred,
    M::HPred,
    M::D45Pred,
    M::D135Pred,
    M::D113Pred,
    M::D157Pred,
    M::D203Pred,
    M::D67Pred,
    M::SmoothPred,
    M::SmoothVPred,
    M::SmoothHPred,
    M::PaethPred,
    M::NearestMv,
    M::NearMv,
    M::GlobalMv,
    M::NewMv,
    M::NearestNearestMv,
    M::NearNearMv,
    M::NearestNewMv,
    M::NewNearestMv,
    M::NearNewMv,
    M::NewNearMv,
    M::GlobalGlobalMv,
    M::NewNewMv,
];

/// The raw discriminants are C's and the table above is indexed by them,
/// so a reordering of either is caught here rather than silently
/// remapping the modulation arms.
#[test]
fn all_modes_is_in_raw_discriminant_order() {
    for (i, m) in ALL_MODES.iter().enumerate() {
        assert_eq!(*m as usize, i, "ALL_MODES[{i}] is {m:?}");
    }
}

/// **The duplicate-transcription pin** (`docs/WORKING-ON-THIS.md` §4:
/// "when you find a second transcription, do not just fix it — PIN IT to
/// the first with a sweep test").
///
/// `DepthWalk::nsq_dev_by_parent_mode` is the copy the live NSQ funnel
/// runs; [`crate::port_md::nsq_skip::modulate_by_parent_mode`] is the
/// reference copy in the general (unwired) port of the same C switch.
/// They must agree on every mode and every threshold the level table can
/// produce — `max_part0_to_part1_dev` is 0/5/20/50/75/80 before the qp
/// scaling, so the sweep covers 0..=100 plus the boundaries where
/// `* 75 / 100` truncates.
#[test]
fn depth_refine_table_agrees_with_port_md_nsq_skip() {
    for dev in 0..=100u32 {
        for m in ALL_MODES {
            let reference = crate::port_md::nsq_skip::modulate_by_parent_mode(dev, m);
            let live = DepthWalk::nsq_dev_by_parent_mode(m as u8, u64::from(dev));
            assert_eq!(
                u64::from(reference),
                live,
                "dev={dev} mode={m:?} (raw {})",
                m as u8
            );
        }
    }
}

/// The three inter arms C has and an intra-only table does not, spelled
/// out as values so a regression names itself. Vectors read straight off
/// `product_coding_loop.c:9867-9895`.
#[test]
fn the_inter_arms_are_the_ones_an_intra_only_table_drops() {
    // `* 75 / 100`, INTER-ONLY — no intra mode reaches this arm.
    assert_eq!(DepthWalk::nsq_dev_by_parent_mode(M::NewMv as u8, 80), 60);
    assert_eq!(DepthWalk::nsq_dev_by_parent_mode(M::NewNewMv as u8, 80), 60);
    // `* 2`, shared with DC/H/V.
    assert_eq!(
        DepthWalk::nsq_dev_by_parent_mode(M::NearestNearestMv as u8, 80),
        160
    );
    assert_eq!(
        DepthWalk::nsq_dev_by_parent_mode(M::NearNearMv as u8, 80),
        160
    );
    // `<< 2`, shared with the directional/smooth intra modes.
    assert_eq!(
        DepthWalk::nsq_dev_by_parent_mode(M::GlobalMv as u8, 80),
        320
    );
    assert_eq!(
        DepthWalk::nsq_dev_by_parent_mode(M::GlobalGlobalMv as u8, 80),
        320
    );
    // `default:` — unchanged. NEARESTMV/NEARMV and the mixed compounds.
    assert_eq!(
        DepthWalk::nsq_dev_by_parent_mode(M::NearestMv as u8, 80),
        80
    );
    assert_eq!(DepthWalk::nsq_dev_by_parent_mode(M::NearMv as u8, 80), 80);
    assert_eq!(
        DepthWalk::nsq_dev_by_parent_mode(M::NearestNewMv as u8, 80),
        80
    );
    // The intra arms are unchanged by the fix — this is what keeps the
    // still path byte-identical by construction.
    assert_eq!(DepthWalk::nsq_dev_by_parent_mode(M::DcPred as u8, 80), 160);
    assert_eq!(
        DepthWalk::nsq_dev_by_parent_mode(M::PaethPred as u8, 80),
        320
    );
}

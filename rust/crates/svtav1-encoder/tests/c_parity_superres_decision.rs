//! The superres denominator DECISION (`Codec/resize.c:1155-1425`).
//!
//! **Tier 1** for the two exported symbols — `nm -g
//! Bin/Release/libSvtAv1Enc.a` reports `T _svt_aom_get_frame_update_type` and
//! `T _svt_aom_get_denom_idx`, and both are driven over their full input
//! domain through `crates/svtav1-cref/shims/refmgmt_shims.c`.
//!
//! **Tier 4** for the five `static` functions, with the derivation written out
//! above each vector. One of them, [`analyze_hor_freq`], rests on the port's
//! forward transform, which is itself tier-1 gated by the transforms lane — so
//! what is hand-derived there is the accumulation and the cumulative pass,
//! not the arithmetic underneath.

use svtav1_cref::ref_mgmt as cref;
use svtav1_encoder::port_picstruct::FrameUpdateType;
use svtav1_encoder::port_superres_decision as sr;

/// TIER 1. `svt_aom_get_frame_update_type` over every input it can take:
/// key/inter x `hierarchical_levels` 0..=6 x `temporal_layer_index` 0..=6.
///
/// This function is a SECOND derivation of the same idea as
/// `set_frame_update_type` (`pd_process.c:4591`), and the two DISAGREE at
/// `hierarchical_levels == 0` — that one splits on `frame_offset % 4` and
/// this one returns `LF_UPDATE` flat. Having the differential means the
/// disagreement is a measured fact about C rather than a reading of it.
#[test]
fn c_parity_frame_update_type_full_domain() {
    let mut distinct = std::collections::HashSet::new();
    for is_key in [false, true] {
        for hier in 0u8..=6 {
            for tl in 0u8..=6 {
                let want = cref::frame_update_type(is_key, hier, tl);
                let got = sr::frame_update_type(is_key, hier, tl) as i32;
                assert_eq!(got, want, "key={is_key} hier={hier} tl={tl}");
                distinct.insert(want);
            }
        }
    }
    assert!(
        distinct.len() >= 4,
        "only {} distinct update types",
        distinct.len()
    );
}

/// TIER 1. `svt_aom_get_denom_idx` over its entire `u8` domain, which is where
/// the `uint8_t` subtraction's wrap below denominator 8 shows up.
#[test]
fn c_parity_denom_idx_full_domain() {
    for d in 0u8..=255 {
        assert_eq!(sr::denom_idx(d), cref::denom_idx(d), "denom {d}");
    }
    assert_eq!(sr::denom_idx(8), 0, "8 is the unscaled denominator");
    assert_eq!(sr::denom_idx(16), 8);
    assert_eq!(sr::denom_idx(7), 255, "below 8 wraps, it does not saturate");
}

/// `get_energy_by_q2_thresh` (`resize.c:1206`) — tier 4.
///
/// Derivation: ARF gets the ARF threshold; a key frame gets the SOLO
/// threshold when it is the only frame before the next key
/// (`frames_to_key <= 1`) and the ordinary one otherwise. Everything else hits
/// C's `assert(0)`, which under NDEBUG returns 0 — a threshold every band
/// exceeds, forcing the maximum denominator. The port refuses instead, and
/// this pins that.
#[test]
fn traced_energy_by_q2_thresh() {
    assert_eq!(
        sr::energy_by_q2_thresh(10, FrameUpdateType::Arf),
        Some(0.008)
    );
    assert_eq!(sr::energy_by_q2_thresh(1, FrameUpdateType::Kf), Some(0.012));
    assert_eq!(sr::energy_by_q2_thresh(0, FrameUpdateType::Kf), Some(0.012));
    assert_eq!(sr::energy_by_q2_thresh(2, FrameUpdateType::Kf), Some(0.008));
    for t in [
        FrameUpdateType::Lf,
        FrameUpdateType::Gf,
        FrameUpdateType::Overlay,
        FrameUpdateType::IntnlOverlay,
        FrameUpdateType::IntnlArf,
    ] {
        assert_eq!(
            sr::energy_by_q2_thresh(10, t),
            None,
            "{t:?} is not a superres frame"
        );
    }
}

/// `av1_superres_in_recode_allowed` (`resize.c:1223`) — tier 4.
///
/// Derivation, and the surprise: the `frames_to_key > 1` half of C's condition
/// is COMMENTED OUT with the note "Empirically found to not be beneficial for
/// image coding", so the predicate really is only `mode == AUTO &&
/// search_type != SOLO`. A port that reinstated the commented clause would
/// silently differ on single-frame encodes.
#[test]
fn traced_superres_in_recode_allowed() {
    let base = sr::SuperresConfig {
        mode: sr::SuperresMode::Auto,
        denom: 8,
        kf_denom: 8,
        qthres: 43,
        kf_qthres: 43,
        auto_search_type: sr::SuperresAutoSearch::All,
    };
    assert!(sr::superres_in_recode_allowed(&base));
    assert!(sr::superres_in_recode_allowed(&sr::SuperresConfig {
        auto_search_type: sr::SuperresAutoSearch::Dual,
        ..base
    }));
    assert!(!sr::superres_in_recode_allowed(&sr::SuperresConfig {
        auto_search_type: sr::SuperresAutoSearch::Solo,
        ..base
    }));
    for mode in [
        sr::SuperresMode::None,
        sr::SuperresMode::Fixed,
        sr::SuperresMode::Random,
        sr::SuperresMode::Qthresh,
    ] {
        assert!(!sr::superres_in_recode_allowed(&sr::SuperresConfig {
            mode,
            ..base
        }));
    }
}

/// `get_superres_denom_from_qindex_energy` (`resize.c:1232`) — tier 4.
///
/// Derivation: `k` starts at `2 * SCALE_NUMERATOR` = 16 and walks down while
/// `energy[k - 1] <= thresh`; the answer is `3 * SCALE_NUMERATOR - k` = `24 -
/// k`. So breaking immediately (k = 16) gives denominator 8 — unscaled — and
/// running all the way to k = 8 gives 16, the narrowest.
///
/// The two synthetic tails below make the walk deterministic without needing a
/// picture: an all-huge tail breaks at once, an all-tiny tail runs to the end,
/// and a tail that is tiny only above band 11 stops there.
#[test]
fn traced_denom_from_qindex_energy_walk() {
    let huge = [1e30f64; 16];
    assert_eq!(
        sr::denom_from_qindex_energy(100, &huge, 0.008, 0.2),
        8,
        "every band exceeds the threshold: break at k = 16"
    );

    // `thresh` is min(threshq * q^2, threshp * energy[1]), so a zero
    // `energy[1]` makes the threshold 0 and every strictly-positive band
    // "exceeds" it. Use an exactly-zero tail to walk the whole way.
    let zero = [0f64; 16];
    assert_eq!(
        sr::denom_from_qindex_energy(100, &zero, 0.008, 0.2),
        16,
        "no band exceeds a zero threshold: k walks down to 8"
    );

    // A tail that is large at and below band 11 and zero above it. The walk
    // stops at the first k with energy[k - 1] > thresh, i.e. k - 1 == 11.
    let mut mixed = [0f64; 16];
    for e in mixed.iter_mut().take(12).skip(1) {
        *e = 1e30;
    }
    assert_eq!(
        sr::denom_from_qindex_energy(100, &mixed, 0.008, 0.2),
        24 - 12
    );
}

/// `analyze_hor_freq` (`resize.c:1155`) — tier 4, and the four traps.
///
/// Derivation 1, the loop bounds: `i < height - 4` and `j < width - 16`, so a
/// picture must be at least 5 rows and 17 columns before ANY block is
/// analysed. At exactly 16 columns or 4 rows, `n` is 0.
///
/// Derivation 2, the `n == 0` sentinel: every band becomes `1e+20`, which is
/// large enough that `denom_from_qindex_energy` breaks immediately and returns
/// the unscaled denominator. A picture too small to analyse is never scaled.
///
/// Derivation 3, the cumulative pass: `k` runs 14 down to 1 adding
/// `energy[k + 1]`, so `energy[15]` is a plain per-band mean and every lower
/// entry is the tail sum at and above it. That makes the array MONOTONICALLY
/// NON-INCREASING in `k`, which is the property the walk in
/// `denom_from_qindex_energy` depends on.
///
/// Derivation 4, DC: entry 0 is never written and stays 0.
#[test]
fn traced_analyze_hor_freq() {
    // Too small in either dimension -> the sentinel.
    for (w, h) in [(16usize, 64usize), (64, 4), (16, 4), (1, 1)] {
        let plane = vec![128u8; w * h];
        let e = sr::analyze_hor_freq(&plane, w, w, h);
        assert_eq!(e[0], 0.0, "DC is never written");
        for k in 1..16 {
            assert_eq!(e[k], 1e+20, "{w}x{h} band {k}");
        }
        // And that sentinel must produce the unscaled denominator.
        assert_eq!(sr::denom_from_qindex_energy(200, &e, 0.008, 0.2), 8);
    }

    // A flat picture has no AC energy at all: every band is exactly 0.
    let w = 64usize;
    let h = 32usize;
    let flat = vec![128u8; w * h];
    let e = sr::analyze_hor_freq(&flat, w, w, h);
    for k in 1..16 {
        assert_eq!(e[k], 0.0, "flat band {k}");
    }

    // A vertical-bar pattern has real horizontal energy, and the cumulative
    // pass must leave the array non-increasing.
    let mut bars = vec![0u8; w * h];
    for (i, p) in bars.iter_mut().enumerate() {
        *p = if (i % w) % 4 < 2 { 16 } else { 235 };
    }
    let e = sr::analyze_hor_freq(&bars, w, w, h);
    assert!(e[1] > 0.0, "a bar pattern must produce energy");
    for k in 1..15 {
        assert!(
            e[k] >= e[k + 1],
            "the cumulative tail must be non-increasing: band {k} = {} < {} = band {}",
            e[k],
            e[k + 1],
            k + 1
        );
    }
    assert_eq!(e[0], 0.0);
}

fn cfg(mode: sr::SuperresMode, search: sr::SuperresAutoSearch) -> sr::SuperresConfig {
    sr::SuperresConfig {
        mode,
        denom: 12,
        kf_denom: 14,
        qthres: 43,
        kf_qthres: 43,
        auto_search_type: search,
    }
}

fn pic(update: FrameUpdateType, qp: u8) -> sr::SuperresPicInput {
    sr::SuperresPicInput {
        allow_intrabc: false,
        allow_screen_content_tools: false,
        is_intra_only: update == FrameUpdateType::Kf,
        picture_qp: qp,
        update_type: update,
        frames_to_key: 32,
    }
}

/// `calc_superres_params` (`resize.c:1311`) — tier 4, mode by mode.
///
/// Derivation, FIXED: a key frame takes `superres_kf_denom` and everything
/// else takes `superres_denom`. That is the trap the superres port map already
/// records from a MEASUREMENT — for a still, `--superres-denom` does nothing
/// and `--superres-kf-denom` is what scales.
///
/// Derivation, the two early exits: `allow_intrabc` returns the default before
/// the switch runs at all, and in QTHRESH `allow_screen_content_tools` breaks
/// out of the case leaving the default. Both leave the denominator unscaled.
#[test]
fn traced_calc_superres_params_modes() {
    let plane = vec![128u8; 64 * 32];
    let mut seed = 34567u32;

    // NONE.
    let d = sr::calc_superres_params(
        &cfg(sr::SuperresMode::None, sr::SuperresAutoSearch::All),
        &pic(FrameUpdateType::Kf, 40),
        &plane,
        64,
        64,
        32,
        &mut seed,
    );
    assert_eq!(d.denom, 8);

    // FIXED: key frame vs not.
    let c = cfg(sr::SuperresMode::Fixed, sr::SuperresAutoSearch::All);
    let kf = sr::calc_superres_params(
        &c,
        &pic(FrameUpdateType::Kf, 40),
        &plane,
        64,
        64,
        32,
        &mut seed,
    );
    assert_eq!(kf.denom, 14, "a key frame takes superres_kf_denom");
    let arf = sr::calc_superres_params(
        &c,
        &pic(FrameUpdateType::Arf, 40),
        &plane,
        64,
        64,
        32,
        &mut seed,
    );
    assert_eq!(arf.denom, 12, "everything else takes superres_denom");

    // allow_intrabc short-circuits before the switch.
    let mut p = pic(FrameUpdateType::Kf, 40);
    p.allow_intrabc = true;
    let d = sr::calc_superres_params(&c, &p, &plane, 64, 64, 32, &mut seed);
    assert_eq!(d.denom, 8, "superres and intra block copy cannot coexist");

    // QTHRESH with screen-content tools on: the case breaks, default survives.
    let cq = cfg(sr::SuperresMode::Qthresh, sr::SuperresAutoSearch::All);
    let mut p = pic(FrameUpdateType::Kf, 63);
    p.allow_screen_content_tools = true;
    let d = sr::calc_superres_params(&cq, &p, &plane, 64, 64, 32, &mut seed);
    assert_eq!(d.denom, 8);

    // QTHRESH below the quantizer threshold: unscaled without analysing.
    let d = sr::calc_superres_params(
        &cq,
        &pic(FrameUpdateType::Kf, 10),
        &plane,
        64,
        64,
        32,
        &mut seed,
    );
    assert_eq!(d.denom, 8);
}

/// `calc_superres_params`, the AUTO recode schedules (`resize.c:1358-1391`).
///
/// Derivation, AUTO_ALL: entries 0..7 are denominators 9..16 and entry 8 is 8
/// (full resolution), the frame takes entry 0, and `superres_total_recode_loop`
/// is 9 — every scale plus unscaled. It fires only for key and ARF frames; any
/// other update type leaves the default untouched even though the quantizer
/// passed the threshold.
///
/// Derivation, AUTO_SOLO: the quantizer threshold is 128 rather than 0, so a
/// low-quantizer frame is left alone where DUAL and ALL would act.
#[test]
fn traced_calc_superres_params_auto_schedules() {
    let plane = vec![128u8; 64 * 32];
    let mut seed = 34567u32;

    let all = cfg(sr::SuperresMode::Auto, sr::SuperresAutoSearch::All);
    let d = sr::calc_superres_params(
        &all,
        &pic(FrameUpdateType::Kf, 40),
        &plane,
        64,
        64,
        32,
        &mut seed,
    );
    assert_eq!(d.denom_array, [9, 10, 11, 12, 13, 14, 15, 16, 8]);
    assert_eq!(d.denom, 9);
    assert_eq!(d.total_recode_loop, 9);

    // A non-superres update type leaves the schedule empty.
    let d = sr::calc_superres_params(
        &all,
        &pic(FrameUpdateType::Lf, 40),
        &plane,
        64,
        64,
        32,
        &mut seed,
    );
    assert_eq!(d.denom, 8);
    assert_eq!(d.total_recode_loop, 0);
    assert_eq!(d.denom_array, [0; 9]);

    // SOLO's threshold is 128, so qp 10 (qindex 40) is below it.
    let solo = cfg(sr::SuperresMode::Auto, sr::SuperresAutoSearch::Solo);
    let d = sr::calc_superres_params(
        &solo,
        &pic(FrameUpdateType::Kf, 10),
        &plane,
        64,
        64,
        32,
        &mut seed,
    );
    assert_eq!(d.denom, 8);
    assert_eq!(d.total_recode_loop, 0);
}

/// `SUPERRES_RANDOM` (`resize.c:1341`) — the denominator is
/// `lcg_rand16(&seed) % 9 + 8`, so it is always in 8..=16, and the sequence is
/// deterministic from C's `static unsigned int seed = 34567`.
///
/// C keeps that seed in a function-level `static`; the port threads it as a
/// `&mut u32` because a hidden mutable static is not expressible in safe Rust,
/// and this test is what that buys — the sequence is now checkable.
#[test]
fn traced_superres_random_sequence() {
    let plane = vec![128u8; 64 * 32];
    let c = cfg(sr::SuperresMode::Random, sr::SuperresAutoSearch::All);

    let mut seed = 34567u32;
    let mut seen = std::collections::HashSet::new();
    let mut first = Vec::new();
    for _ in 0..64 {
        let d = sr::calc_superres_params(
            &c,
            &pic(FrameUpdateType::Kf, 40),
            &plane,
            64,
            64,
            32,
            &mut seed,
        );
        assert!(
            (8..=16).contains(&d.denom),
            "denominator {} out of range",
            d.denom
        );
        seen.insert(d.denom);
        first.push(d.denom);
    }
    assert!(
        seen.len() > 3,
        "the generator produced only {} values",
        seen.len()
    );

    // Deterministic from the seed: a second run from 34567 repeats it exactly.
    let mut seed = 34567u32;
    let second: Vec<u8> = (0..64)
        .map(|_| {
            sr::calc_superres_params(
                &c,
                &pic(FrameUpdateType::Kf, 40),
                &plane,
                64,
                64,
                32,
                &mut seed,
            )
            .denom
        })
        .collect();
    assert_eq!(first, second);
}

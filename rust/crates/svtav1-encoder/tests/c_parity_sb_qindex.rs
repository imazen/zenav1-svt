//! Differential parity for the delta-q L2 building blocks:
//! the sub-sampled 8x8 mean/mean-square producers vs the exported C
//! functions, plus a whole-SB variance cross-check assembled from them,
//! plus the delta-q normalizer (`svt_av1_normalize_sb_delta_q`) that makes
//! the pack's truncating delta-q divide exact.
use svtav1_cref as cref;
use svtav1_encoder::sb_qindex;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
}

#[test]
fn sub_mean_producers_match_c() {
    let mut rng = Rng(0x5b91dec5);
    for _ in 0..200 {
        let stride = 8 + (rng.next() as usize % 24);
        let buf: Vec<u8> = (0..stride * 8).map(|_| (rng.next() >> 32) as u8).collect();
        // Rust-side recomputation with the sb_qindex internal formula via
        // compute_sb_variances on an 8x8-only view is indirect; instead
        // recompute here exactly and pin BOTH against C.
        let mut s: u64 = 0;
        let mut sq: u64 = 0;
        for vi in 0..4 {
            for hi in 0..8 {
                let p = u64::from(buf[(2 * vi) * stride + hi]);
                s += p;
                sq += p * p;
            }
        }
        assert_eq!(s << 3, cref::sub_mean_8x8(&buf, stride as u16));
        assert_eq!(sq << 11, cref::sub_mean_squared_8x8(&buf, stride as u32));
    }
}

#[test]
fn sb_variance_producer_consistent_with_c_blocks() {
    // Assemble a full 64x64 SB and verify compute_sb_variances' 8x8 level
    // equals the C producers combined with the fork SVT_VAR_STORE formula.
    let mut rng = Rng(0xfeed5b);
    let stride = 80usize;
    let buf: Vec<u8> = (0..stride * 64).map(|_| (rng.next() >> 32) as u8).collect();
    let v = sb_qindex::compute_sb_variances(&buf, stride, 64, 64, 0, 0);
    for row in 0..8 {
        for col in 0..8 {
            let blk = &buf[row * 8 * stride + col * 8..];
            let m = cref::sub_mean_8x8(blk, stride as u16);
            let sq = cref::sub_mean_squared_8x8(blk, stride as u32);
            let expect = (sq as i64 - (m * m) as i64) as f64 / 65536.0;
            assert_eq!(v.var_8x8[row * 8 + col], expect, "blk ({row},{col})");
        }
    }
}

/// EXHAUSTIVE differential vs the real exported `svt_av1_normalize_sb_delta_q`
/// (rc_aq.c:830) over every (base_q_idx, delta_q_res, sb_qindex) triple the
/// encoder can reach: base 1..=255 x res {2,4,8} x every input qindex 1..=255.
/// One shim call per (base, res) carries all 255 SBs, so C's per-SB loop is
/// exercised as a loop too, not just as a scalar.
#[test]
fn normalize_sb_delta_q_matches_c_exhaustive() {
    for res in [2u8, 4, 8] {
        for base in 1u8..=255 {
            let inputs: Vec<u8> = (1u8..=255).collect();

            let mut c_out = inputs.clone();
            cref::normalize_sb_delta_q(base, res, &mut c_out);

            let mut rs: Vec<i32> = inputs.iter().map(|&q| i32::from(q)).collect();
            sb_qindex::normalize_sb_delta_q(base, res, &mut rs);
            // The port's helper works in i32 while C's `sb_ptr->qindex` is a
            // uint8_t. Prove the port stays inside the u8 domain BEFORE the
            // narrowing cast below, so a hypothetical out-of-range value can
            // never be laundered into a match by wrapping.
            assert!(
                rs.iter().all(|&q| (0..=255).contains(&q)),
                "base {base} res {res}: port produced a qindex outside 0..=255: {rs:?}"
            );
            let rs_out: Vec<u8> = rs.iter().map(|&q| q as u8).collect();

            assert_eq!(rs_out, c_out, "base {base} res {res}");

            // The property the whole function exists for: after normalization
            // every SB is congruent to the frame base mod delta_q_res, so the
            // pack's TRUNCATING `(cur - prev) / res` (entropy_coding.c:5002) is
            // exact. Assert it on the C output too — that pins the property to
            // the reference, not to our port's idea of it.
            for (i, &q) in c_out.iter().enumerate() {
                assert_eq!(
                    (i32::from(q) - i32::from(base)).rem_euclid(i32::from(res)),
                    0,
                    "C output not in base residue class: base {base} res {res} sb {i} q {q}"
                );
            }
        }
    }
}

/// Whole-plan differential: run the mainline variance-boost chain, then check
/// that the plan it emits survives a faithful simulation of the C pack's
/// truncating delta-q writer (entropy_coding.c:4996-5015) and the decoder's
/// accumulator (spec 5.11.41: `prev = prev + reduced * delta_q_res`).
///
/// This is the corruption witness in miniature: without the normalizer the two
/// accumulators diverge and the error COMPOUNDS across the SB raster, so a
/// later SB's decoded qindex can be far from the encoder's.
///
/// EVIDENCE TIER: unlike `normalize_sb_delta_q_matches_c_exhaustive` above,
/// this test calls NO C function — the pack/decoder pair is hand-simulated
/// from entropy_coding.c:4996-5015 and spec 5.11.41. It also deliberately
/// simulates a delta at EVERY SB, whereas C (and pipeline.rs:4586-4599) emit
/// one only when `super_block_upper_left && (bsize != sb_size || !skip)`;
/// that is the strictly harder condition, so a pass here implies a pass under
/// C's guard. The end-to-end aomdec check is
/// `svtav1/examples/variance_boost_recon.rs`.
#[test]
fn mainline_plan_survives_c_pack_decoder_roundtrip() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    for cli_qp in [20u8, 30, 40, 55, 63] {
        // Random-but-reproducible per-SB integer variance maps: a spread of
        // flat and textured SBs so the boost produces a real (non-uniform) plan.
        let vars: Vec<svtav1_encoder::pd0::SbVariance> = (0..24)
            .map(|_| {
                let mut v = [0u16; 85];
                let scale = (rng.next() % 4096) as u16;
                for x in v.iter_mut() {
                    *x = ((rng.next() % 64) as u16).saturating_add(scale);
                }
                svtav1_encoder::pd0::SbVariance(v)
            })
            .collect();

        let base = svtav1_encoder::rate_control::qp_to_qindex(cli_qp);
        let plan = sb_qindex::variance_adjust_qp_mainline(base, &vars, 3, 6, 2, cli_qp, 8);
        let res = i32::from(plan.delta_q_res);
        assert_eq!(
            plan.base_qindex, base,
            "mainline must not resignal the base"
        );
        // Anti-vacuity: every swept qp must actually reduce the delta-q
        // resolution, otherwise the truncating divide below is trivially exact
        // and proves nothing. qindex 80/120/160/220/255 -> res 2/4/8/8/8.
        assert_ne!(res, 1, "cli_qp {cli_qp} would make this cell vacuous");
        // ... and the plan must be non-uniform, or every delta is 0.
        assert!(
            plan.sb_qindex.iter().any(|&q| q != plan.sb_qindex[0]),
            "cli_qp {cli_qp}: uniform plan is a vacuous cell"
        );

        // Simulate the pack + decoder over the SB raster.
        let mut enc_prev = i32::from(plan.base_qindex);
        let mut dec_prev = i32::from(plan.base_qindex);
        for (i, &q) in plan.sb_qindex.iter().enumerate() {
            let cur = i32::from(q);
            let reduced = (cur - enc_prev) / res; // C: truncating integer divide
            enc_prev = cur; // C: prev_qindex[tile] = current_q_index
            dec_prev += reduced * res; // decoder accumulator
            assert_eq!(
                dec_prev, cur,
                "qp {cli_qp} res {res} sb {i}: decoder reconstructs {dec_prev}, \
                 encoder used {cur} (delta-q normalization missing/incorrect)"
            );
        }
    }
}

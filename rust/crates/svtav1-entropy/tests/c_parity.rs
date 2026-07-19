//! Differential parity tests: svtav1-entropy vs the C reference implementation.
//!
//! Every test drives the Rust port and the actual C SVT-AV1 code (linked via
//! `svtav1-cref`) on identical inputs and asserts bit-for-bit identical
//! results — both the emitted bytes and the adapted CDF state.

use svtav1_cref as cref;
use svtav1_entropy::cdf::update_cdf;
use svtav1_entropy::range_coder::OdEcEnc;

/// Deterministic xorshift64* PRNG — no external deps, reproducible failures.
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

    /// Uniform in `[0, n)`.
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Generate a random valid CDF in C layout for `nsymbs` symbols:
/// strictly decreasing ICDF values, structural 0 at `[nsymbs-1]`, and a
/// random adaptation counter (0..=32) at `[nsymbs]`.
///
/// With probability ~1/4 the distribution is made extremely skewed so that
/// long runs of the dominant symbol exercise carry propagation.
fn random_cdf(rng: &mut Rng, nsymbs: usize) -> Vec<u16> {
    let mut cdf = vec![0u16; nsymbs + 1];
    let skewed = rng.below(4) == 0;
    loop {
        // Draw nsymbs-1 distinct cut points in (0, 32768), descending.
        let mut cuts: Vec<u16> = (0..nsymbs - 1)
            .map(|_| {
                if skewed {
                    // Cluster mass at one end: values near 1 or near 32767.
                    if rng.below(2) == 0 {
                        1 + rng.below(64) as u16
                    } else {
                        32767 - rng.below(64) as u16
                    }
                } else {
                    1 + rng.below(32766) as u16
                }
            })
            .collect();
        cuts.sort_unstable_by(|a, b| b.cmp(a));
        cuts.dedup();
        if cuts.len() == nsymbs - 1 {
            cdf[..nsymbs - 1].copy_from_slice(&cuts);
            break;
        }
    }
    cdf[nsymbs - 1] = 0; // structural zero
    cdf[nsymbs] = rng.below(33) as u16; // adaptation counter
    cdf
}

#[test]
fn update_cdf_matches_c() {
    let mut rng = Rng(0x9E3779B97F4A7C15);
    for nsymbs in 2..=16usize {
        for trial in 0..2000 {
            let start = random_cdf(&mut rng, nsymbs);
            let val = rng.below(nsymbs as u64) as usize;

            let mut rust = start.clone();
            update_cdf(&mut rust, val, nsymbs);

            let mut c = start.clone();
            cref::update_cdf(&mut c, val, nsymbs);

            assert_eq!(
                rust, c,
                "update_cdf diverged: nsymbs={nsymbs} val={val} trial={trial} start={start:?}"
            );
        }
    }
}

#[test]
fn ec_static_stream_matches_c() {
    let mut rng = Rng(0xA5A5_5A5A_1234_5678);
    for trial in 0..200 {
        let len = 1 + rng.below(768) as usize;
        let mut rust = OdEcEnc::new(64);
        let mut c = cref::RefEcEnc::new(64);

        for step in 0..len {
            if rng.below(4) == 0 {
                // Boolean path.
                let f = 1 + rng.below(32766) as u32;
                let val = rng.below(2) == 1;
                rust.encode_bool_q15(val, f);
                c.encode_bool_q15(val, f);
            } else {
                let nsymbs = 2 + rng.below(15) as usize;
                let cdf = random_cdf(&mut rng, nsymbs);
                let s = rng.below(nsymbs as u64) as usize;
                rust.encode_cdf_q15(s, &cdf, nsymbs);
                c.encode_cdf_q15(s, &cdf, nsymbs);
            }
            if step % 97 == 0 {
                assert_eq!(
                    rust.tell(),
                    c.tell() as i32,
                    "tell() diverged at trial={trial} step={step}"
                );
            }
        }

        let c_bytes = c.done();
        let rust_bytes = rust.done().to_vec();
        assert_eq!(
            rust_bytes, c_bytes,
            "static stream bytes diverged at trial={trial} len={len}"
        );
    }
}

#[test]
fn ec_adaptive_stream_matches_c() {
    // The real write path: symbols encoded through adapting CDF contexts.
    // Both sides maintain their own copies; final bytes AND final CDF states
    // must be identical.
    let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
    for trial in 0..100 {
        // A pool of adapting contexts of varying alphabet sizes.
        let nctx = 1 + rng.below(6) as usize;
        let mut ctxs: Vec<(usize, Vec<u16>)> = (0..nctx)
            .map(|_| {
                let nsymbs = 2 + rng.below(15) as usize;
                let mut cdf = random_cdf(&mut rng, nsymbs);
                cdf[nsymbs] = 0; // counters start at 0 like fresh contexts
                (nsymbs, cdf)
            })
            .collect();
        let mut c_ctxs = ctxs.clone();

        let mut rust = OdEcEnc::new(0); // also exercises zero-capacity growth
        let mut c = cref::RefEcEnc::new(0);

        let len = 1 + rng.below(2048) as usize;
        for _ in 0..len {
            let k = rng.below(nctx as u64) as usize;
            let (nsymbs, ref mut cdf) = ctxs[k];
            let s = rng.below(nsymbs as u64) as usize;
            // The real write path: encode then adapt (aom_write_symbol).
            rust.encode_cdf_q15(s, cdf, nsymbs);
            update_cdf(cdf, s, nsymbs);
            let (c_nsymbs, ref mut c_cdf) = c_ctxs[k];
            debug_assert_eq!(nsymbs, c_nsymbs);
            c.write_symbol(s, c_cdf, c_nsymbs);
        }

        let c_bytes = c.done();
        let rust_bytes = rust.done().to_vec();
        assert_eq!(
            rust_bytes, c_bytes,
            "adaptive stream bytes diverged at trial={trial}"
        );
        for (k, (r, c)) in ctxs.iter().zip(c_ctxs.iter()).enumerate() {
            assert_eq!(r.1, c.1, "context {k} CDF state diverged at trial={trial}");
        }
    }
}

#[test]
fn ec_carry_torture_matches_c() {
    // Deliberate carry chains: a dominant symbol keeps `low` near the top of
    // the interval, producing 0xFF runs; an occasional improbable symbol then
    // triggers backward carry propagation through those runs.
    for (seed, dominant_first) in [(1u64, true), (2, false), (3, true), (4, false)] {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        // Extreme 2-symbol CDFs in C layout.
        // icdf[0]=1     -> P(sym0) ~ 1        (dominant first symbol)
        // icdf[0]=32767 -> P(sym0) ~ 1/32768  (dominant second symbol)
        let cdf: [u16; 3] = if dominant_first {
            [1, 0, 0]
        } else {
            [32767, 0, 0]
        };
        let dominant = usize::from(!dominant_first);

        let mut rust = OdEcEnc::new(16);
        let mut c = cref::RefEcEnc::new(16);
        for _ in 0..4096 {
            let s = if rng.below(97) == 0 {
                1 - dominant
            } else {
                dominant
            };
            rust.encode_cdf_q15(s, &cdf, 2);
            c.encode_cdf_q15(s, &cdf, 2);
        }
        let c_bytes = c.done();
        let rust_bytes = rust.done().to_vec();
        assert_eq!(rust_bytes, c_bytes, "carry torture diverged (seed={seed})");
    }
}

#[test]
fn ec_empty_and_tiny_streams_match_c() {
    // Zero symbols.
    let mut rust = OdEcEnc::new(8);
    let mut c = cref::RefEcEnc::new(8);
    let c_bytes = c.done();
    assert_eq!(rust.done().to_vec(), c_bytes, "empty stream diverged");

    // One symbol, every alphabet size, every symbol index.
    for nsymbs in 2..=16usize {
        let mut rng = Rng(nsymbs as u64 * 7919 + 1);
        for s in 0..nsymbs {
            let cdf = random_cdf(&mut rng, nsymbs);
            let mut rust = OdEcEnc::new(8);
            let mut c = cref::RefEcEnc::new(8);
            rust.encode_cdf_q15(s, &cdf, nsymbs);
            c.encode_cdf_q15(s, &cdf, nsymbs);
            let c_bytes = c.done();
            assert_eq!(
                rust.done().to_vec(),
                c_bytes,
                "single-symbol stream diverged: nsymbs={nsymbs} s={s}"
            );
        }
    }
}

/// The committed default CDF tables must exactly match a fresh extraction
/// from the linked C library (guards against upstream bumps and manual
/// edits of the generated file).
#[test]
fn c_default_cdf_tables_match() {
    use svtav1_entropy::default_cdfs as d;

    // Q-dependent coefficient tables: bucket k extracted at its
    // representative qindex.
    let q_reps = [10, 40, 100, 160];
    for (k, &q) in q_reps.iter().enumerate() {
        cref::fc_init(q);
        macro_rules! check_q {
            ($table:expr, $c:expr, $name:literal) => {{
                let rust_flat: Vec<u16> = $table.iter().flatten().flatten().copied().collect();
                assert_eq!(rust_flat, $c, concat!($name, " bucket mismatch"));
            }};
        }
        check_q!(
            d::TXB_SKIP_CDF[k],
            cref::fc_table(cref::FcTable::TxbSkip),
            "TXB_SKIP_CDF"
        );
        check_q!(
            d::DC_SIGN_CDF[k],
            cref::fc_table(cref::FcTable::DcSign),
            "DC_SIGN_CDF"
        );
        check_q!(
            d::EOB_FLAG_CDF16[k],
            cref::fc_table(cref::FcTable::EobFlag16),
            "EOB_FLAG_CDF16"
        );
        check_q!(
            d::EOB_FLAG_CDF1024[k],
            cref::fc_table(cref::FcTable::EobFlag1024),
            "EOB_FLAG_CDF1024"
        );
        let base: Vec<u16> = d::COEFF_BASE_CDF[k]
            .iter()
            .flatten()
            .flatten()
            .flatten()
            .copied()
            .collect();
        assert_eq!(
            base,
            cref::fc_table(cref::FcTable::CoeffBase),
            "COEFF_BASE_CDF bucket {q}"
        );
        let br: Vec<u16> = d::COEFF_BR_CDF[k]
            .iter()
            .flatten()
            .flatten()
            .flatten()
            .copied()
            .collect();
        assert_eq!(
            br,
            cref::fc_table(cref::FcTable::CoeffBr),
            "COEFF_BR_CDF bucket {q}"
        );
        let beob: Vec<u16> = d::COEFF_BASE_EOB_CDF[k]
            .iter()
            .flatten()
            .flatten()
            .flatten()
            .copied()
            .collect();
        assert_eq!(
            beob,
            cref::fc_table(cref::FcTable::CoeffBaseEob),
            "COEFF_BASE_EOB_CDF bucket {q}"
        );
        let eex: Vec<u16> = d::EOB_EXTRA_CDF[k]
            .iter()
            .flatten()
            .flatten()
            .flatten()
            .copied()
            .collect();
        assert_eq!(
            eex,
            cref::fc_table(cref::FcTable::EobExtra),
            "EOB_EXTRA_CDF bucket {q}"
        );
    }

    // Mode tables (q-independent).
    cref::fc_init(60);
    let part: Vec<u16> = d::PARTITION_CDF.iter().flatten().copied().collect();
    assert_eq!(
        part,
        cref::fc_table(cref::FcTable::Partition),
        "PARTITION_CDF"
    );
    let skip: Vec<u16> = d::SKIP_CDF.iter().flatten().copied().collect();
    assert_eq!(skip, cref::fc_table(cref::FcTable::Skip), "SKIP_CDF");
    let kf: Vec<u16> = d::KF_Y_CDF.iter().flatten().flatten().copied().collect();
    assert_eq!(kf, cref::fc_table(cref::FcTable::KfY), "KF_Y_CDF");
    let ad: Vec<u16> = d::ANGLE_DELTA_CDF.iter().flatten().copied().collect();
    assert_eq!(
        ad,
        cref::fc_table(cref::FcTable::AngleDelta),
        "ANGLE_DELTA_CDF"
    );
    let iet: Vec<u16> = d::INTRA_EXT_TX_CDF
        .iter()
        .flatten()
        .flatten()
        .flatten()
        .copied()
        .collect();
    assert_eq!(
        iet,
        cref::fc_table(cref::FcTable::IntraExtTx),
        "INTRA_EXT_TX_CDF"
    );
    let uv: Vec<u16> = d::UV_MODE_CDF.iter().flatten().flatten().copied().collect();
    assert_eq!(uv, cref::fc_table(cref::FcTable::UvMode), "UV_MODE_CDF");
    let ts: Vec<u16> = d::TX_SIZE_CDF.iter().flatten().flatten().copied().collect();
    assert_eq!(ts, cref::fc_table(cref::FcTable::TxSize), "TX_SIZE_CDF");
    assert_eq!(
        d::FILTER_INTRA_MODE_CDF.to_vec(),
        cref::fc_table(cref::FcTable::FilterIntraMode)
    );
    assert_eq!(
        d::DELTA_Q_CDF.to_vec(),
        cref::fc_table(cref::FcTable::DeltaQ)
    );
    assert_eq!(
        d::INTRABC_CDF.to_vec(),
        cref::fc_table(cref::FcTable::IntraBc)
    );
    let fi: Vec<u16> = d::FILTER_INTRA_CDF.iter().flatten().copied().collect();
    assert_eq!(
        fi,
        cref::fc_table(cref::FcTable::FilterIntra),
        "FILTER_INTRA_CDF"
    );
    let ym: Vec<u16> = d::Y_MODE_CDF.iter().flatten().copied().collect();
    assert_eq!(ym, cref::fc_table(cref::FcTable::YMode), "Y_MODE_CDF");
}

// ---- coeff_c helper parity ----

#[test]
fn coeff_c_dims_and_scans_match_c() {
    use svtav1_entropy::{coeff_c, scan_tables};
    for ts in 0..19usize {
        assert_eq!(coeff_c::txb_bwl(ts), cref::txb_bwl(ts), "bwl ts={ts}");
        assert_eq!(coeff_c::txb_wide(ts), cref::txb_wide(ts), "wide ts={ts}");
        assert_eq!(coeff_c::txb_high(ts), cref::txb_high(ts), "high ts={ts}");
        assert_eq!(
            coeff_c::txsize_entropy_ctx(ts),
            cref::txsize_entropy_ctx(ts),
            "txs_ctx ts={ts}"
        );
        for class in 0..3usize {
            let c_scan: Vec<u16> = cref::scan(ts, class).iter().map(|&v| v as u16).collect();
            assert_eq!(
                scan_tables::scan(ts, class),
                &c_scan[..],
                "scan ts={ts} class={class}"
            );
        }
        // 2D nz-map context offsets: our generator formula vs the C table.
        let n = cref::scan_len(ts);
        for idx in 0..n {
            assert_eq!(
                coeff_c::nz_map_ctx_offset_2d(ts, idx) as i32,
                cref::nz_map_ctx_offset(ts, idx),
                "nz_map_ctx_offset ts={ts} idx={idx}"
            );
        }
    }
    for t in 0..16usize {
        assert_eq!(
            scan_tables::TX_TYPE_TO_SCAN_INDEX[t] as usize,
            cref::tx_type_to_scan_index(t),
            "scan index tx_type={t}"
        );
    }
}

#[test]
fn coeff_c_eob_pos_token_matches_c() {
    use svtav1_entropy::coeff_c;
    for eob in 1..=1024i32 {
        let (t, extra) = coeff_c::eob_pos_token(eob);
        let (ct, cextra) = cref::get_eob_pos_token(eob);
        assert_eq!((t as i32, extra), (ct, cextra), "eob={eob}");
    }
}

#[test]
fn coeff_c_levels_and_contexts_match_c() {
    use svtav1_entropy::coeff_c;
    let mut rng = Rng(0xC0FFEE_D00D_1234);
    // All square/rect sizes after adjustment; classes 2D/H/V via tx types.
    for ts in 0..19usize {
        let width = coeff_c::txb_wide(ts);
        let height = coeff_c::txb_high(ts);
        let n = width * height;
        for &tx_type in &[0usize, 10, 11] {
            // DCT_DCT (2D), V_DCT (VERT), H_DCT (HORIZ)
            let tx_class = coeff_c::TX_TYPE_TO_CLASS[tx_type];
            for _trial in 0..8 {
                // Sparse random coefficients.
                let mut coeffs = vec![0i32; n];
                let nnz = 1 + rng.below((n as u64 / 4).max(1)) as usize;
                for _ in 0..nnz {
                    let p = rng.below(n as u64) as usize;
                    let mag = 1 + rng.below(300) as i32;
                    coeffs[p] = if rng.below(2) == 0 { mag } else { -mag };
                }

                // Level map parity.
                let mut rust_buf = [0u8; coeff_c::TX_PAD_2D];
                coeff_c::txb_init_levels(&coeffs, width, height, &mut rust_buf);
                let origin = coeff_c::levels_origin(width);
                let stride = width + coeff_c::TX_PAD_HOR;
                let written =
                    stride * height + coeff_c::TX_PAD_BOTTOM * stride + coeff_c::TX_PAD_END;
                let mut c_buf = [0u8; coeff_c::TX_PAD_2D];
                cref::txb_init_levels(&coeffs, width, height, &mut c_buf[origin..]);
                assert_eq!(
                    &rust_buf[origin..origin + written],
                    &c_buf[origin..origin + written],
                    "levels ts={ts}"
                );

                // Scan + eob from the level map.
                let scan = svtav1_entropy::scan_tables::scan(
                    ts,
                    svtav1_entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[tx_type] as usize,
                );
                let mut eob = 0usize;
                for (i, &pos) in scan.iter().enumerate() {
                    if coeffs[pos as usize] != 0 {
                        eob = i + 1;
                    }
                }
                if eob == 0 {
                    continue;
                }

                // nz-map contexts parity.
                let mut rust_ctx = [0i8; 32 * 32];
                coeff_c::get_nz_map_contexts(&rust_buf, scan, eob, ts, tx_class, &mut rust_ctx);
                let c_scan: Vec<i16> = scan.iter().map(|&v| v as i16).collect();
                let mut c_ctx = [0i8; 32 * 32];
                cref::get_nz_map_contexts(
                    &c_buf[origin..],
                    &c_scan,
                    eob as u16,
                    ts,
                    tx_class,
                    &mut c_ctx,
                );
                assert_eq!(&rust_ctx[..n], &c_ctx[..n], "nz ctx ts={ts} type={tx_type}");

                // br context parity at every nonzero position.
                let bwl = coeff_c::txb_bwl(ts);
                for &pos in scan[..eob].iter() {
                    let r = coeff_c::br_ctx(&rust_buf, pos as usize, bwl, tx_class);
                    let c = cref::get_br_ctx(&c_buf[origin..], pos as usize, bwl, tx_class);
                    assert_eq!(r as i32, c, "br_ctx ts={ts} pos={pos}");
                }
            }
        }
    }
}

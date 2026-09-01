//! Differential parity: loop-restoration tap coding chain
//! (`svt_aom_write_primitive_refsubexpfin` / count) vs the C reference,
//! exhaustively over the exact (n, k) pairs the wiener taps use.

use svtav1_cref as cref;
use svtav1_encoder::entropy::lr;
use svtav1_encoder::entropy::writer::AomWriter;

/// Coded BYTES and bit counts must match C for every (ref, v) pair of every
/// tap alphabet. Byte equality through a fresh od_ec coder pins the whole
/// chain: recenter_finite_nonneg -> subexpfin -> quniform -> literal/bool.
#[test]
fn refsubexpfin_matches_c() {
    let cases: [(u16, u16); 3] = [
        (
            (lr::WIENER_FILT_TAP0_MAXV - lr::WIENER_FILT_TAP0_MINV + 1) as u16,
            lr::WIENER_FILT_TAP0_SUBEXP_K,
        ),
        (
            (lr::WIENER_FILT_TAP1_MAXV - lr::WIENER_FILT_TAP1_MINV + 1) as u16,
            lr::WIENER_FILT_TAP1_SUBEXP_K,
        ),
        (
            (lr::WIENER_FILT_TAP2_MAXV - lr::WIENER_FILT_TAP2_MINV + 1) as u16,
            lr::WIENER_FILT_TAP2_SUBEXP_K,
        ),
    ];
    for &(n, k) in &cases {
        for r in 0..n {
            for v in 0..n {
                let c_bytes = cref::write_refsubexpfin_bytes(n, k, r, v);
                let mut w = AomWriter::new(64);
                lr::write_primitive_refsubexpfin(&mut w, n, k, r, v);
                let r_bytes = w.done().to_vec();
                assert_eq!(
                    c_bytes, r_bytes,
                    "refsubexpfin bytes diverge n={n} k={k} ref={r} v={v}"
                );
                let c_count = cref::count_refsubexpfin(n, k, r, v);
                let r_count = lr::count_primitive_refsubexpfin(n, k, r, v);
                assert_eq!(c_count, r_count, "count diverges n={n} k={k} ref={r} v={v}");
            }
        }
    }
}

/// `count_wiener_bits` (the search's rate term) must equal the C count sum
/// for random tap/ref pairs, both window sizes. The C reference is composed
/// from the same count primitive proven above, so this test pins OUR
/// composition (tap order, alphabets, win-5 TAP0 skip) against it.
#[test]
fn count_wiener_bits_composition() {
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
        fn tap(&mut self, min: i32, max: i32) -> i16 {
            (min + (self.next() % (max - min + 1) as u64) as i32) as i16
        }
    }
    let mut rng = Rng(0xC0DE_0001);
    let rand_filter = |rng: &mut Rng, win5: bool| -> [i16; 8] {
        let t0 = if win5 {
            0
        } else {
            rng.tap(lr::WIENER_FILT_TAP0_MINV, lr::WIENER_FILT_TAP0_MAXV)
        };
        let t1 = rng.tap(lr::WIENER_FILT_TAP1_MINV, lr::WIENER_FILT_TAP1_MAXV);
        let t2 = rng.tap(lr::WIENER_FILT_TAP2_MINV, lr::WIENER_FILT_TAP2_MAXV);
        [t0, t1, t2, -2 * (t0 + t1 + t2), t2, t1, t0, 0]
    };
    for iter in 0..500 {
        let win5 = iter % 2 == 0;
        let win = if win5 {
            lr::WIENER_WIN_CHROMA
        } else {
            lr::WIENER_WIN
        };
        let v = rand_filter(&mut rng, win5);
        let h = rand_filter(&mut rng, win5);
        let rv = rand_filter(&mut rng, win5);
        let rh = rand_filter(&mut rng, win5);

        let mut expect = 0i32;
        let tap_bits = |f: &[i16; 8], rf: &[i16; 8], idx: usize, minv: i32, maxv: i32, k: u16| {
            cref::count_refsubexpfin(
                (maxv - minv + 1) as u16,
                k,
                (rf[idx] as i32 - minv) as u16,
                (f[idx] as i32 - minv) as u16,
            )
        };
        if !win5 {
            expect += tap_bits(
                &v,
                &rv,
                0,
                lr::WIENER_FILT_TAP0_MINV,
                lr::WIENER_FILT_TAP0_MAXV,
                lr::WIENER_FILT_TAP0_SUBEXP_K,
            );
        }
        expect += tap_bits(
            &v,
            &rv,
            1,
            lr::WIENER_FILT_TAP1_MINV,
            lr::WIENER_FILT_TAP1_MAXV,
            lr::WIENER_FILT_TAP1_SUBEXP_K,
        );
        expect += tap_bits(
            &v,
            &rv,
            2,
            lr::WIENER_FILT_TAP2_MINV,
            lr::WIENER_FILT_TAP2_MAXV,
            lr::WIENER_FILT_TAP2_SUBEXP_K,
        );
        if !win5 {
            expect += tap_bits(
                &h,
                &rh,
                0,
                lr::WIENER_FILT_TAP0_MINV,
                lr::WIENER_FILT_TAP0_MAXV,
                lr::WIENER_FILT_TAP0_SUBEXP_K,
            );
        }
        expect += tap_bits(
            &h,
            &rh,
            1,
            lr::WIENER_FILT_TAP1_MINV,
            lr::WIENER_FILT_TAP1_MAXV,
            lr::WIENER_FILT_TAP1_SUBEXP_K,
        );
        expect += tap_bits(
            &h,
            &rh,
            2,
            lr::WIENER_FILT_TAP2_MINV,
            lr::WIENER_FILT_TAP2_MAXV,
            lr::WIENER_FILT_TAP2_SUBEXP_K,
        );

        assert_eq!(
            lr::count_wiener_bits(win, &v, &h, &rv, &rh),
            expect,
            "iter {iter} win {win}"
        );
    }
}

/// `write_wiener_filter` byte stream = the six (four for win-5) refsubexpfin
/// writes in C order, and it must update the reference filters in place.
#[test]
fn write_wiener_filter_stream_and_ref_chain() {
    // Two units back-to-back: the second codes against the first's taps.
    let v1: [i16; 8] = [0, 8, -17, 18, -17, 8, 0, 0];
    let h1: [i16; 8] = [0, -8, 26, -36, 26, -8, 0, 0];
    let v2: [i16; 8] = [0, -5, 12, -14, 12, -5, 0, 0];
    let h2: [i16; 8] = [0, 3, 40, -86, 40, 3, 0, 0];

    let mut w = AomWriter::new(256);
    let mut ref_v = [3, -7, 15, -2 * (3 - 7 + 15), 15, -7, 3, 0]; // set_default_wiener
    let mut ref_h = ref_v;
    lr::write_wiener_filter(
        &mut w,
        lr::WIENER_WIN_CHROMA,
        &v1,
        &h1,
        &mut ref_v,
        &mut ref_h,
    );
    assert_eq!(ref_v, v1);
    assert_eq!(ref_h, h1);
    lr::write_wiener_filter(
        &mut w,
        lr::WIENER_WIN_CHROMA,
        &v2,
        &h2,
        &mut ref_v,
        &mut ref_h,
    );
    assert_eq!(ref_v, v2);
    assert_eq!(ref_h, h2);
    let bytes = w.done().to_vec();
    assert!(!bytes.is_empty());
}

/// The SGR unit writer, `write_sgrproj_filter` (entropy_coding.c:4126).
///
/// EVIDENCE: the C body is `static`, so `cref::write_sgrproj_filter_bytes`
/// composes the exported primitives that body calls (`aom_write_literal` and
/// `svt_aom_write_primitive_refsubexpfin`) over C's own
/// `svt_aom_eb_sgr_params` table. Byte equality through a fresh od_ec coder
/// therefore pins the `ep` literal width, the (n, k) alphabets, WHICH `xqd`
/// components are signalled for each radius shape, and their order — the four
/// ways this writer can be wrong — with only the radius dispatch itself
/// transcribed on both sides.
///
/// Anti-vacuity: the three radius shapes are counted and all three must be
/// exercised, so a table transcription that collapsed them would fail here
/// rather than pass with one arm never taken.
#[test]
fn write_sgrproj_filter_matches_c() {
    use svtav1_dsp::port_sgr::{
        SGR_PARAMS, SGRPROJ_PRJ_MAX0, SGRPROJ_PRJ_MAX1, SGRPROJ_PRJ_MIN0, SGRPROJ_PRJ_MIN1,
    };
    use svtav1_encoder::port_sgr_search::SgrprojInfo;

    // The reference every unit chains its first delta against.
    assert_eq!(
        SgrprojInfo::c_default().xqd,
        cref::default_sgrproj_xqd(),
        "set_default_sgrproj must match C — a wrong reference shifts every coded delta"
    );

    let mut shapes = [0usize; 3]; // r0==0 / r1==0 / both live
    let mut cells = 0usize;
    for ep in 0..SGR_PARAMS.len() {
        let p = SGR_PARAMS[ep];
        let shape = if p.r[0] == 0 {
            0
        } else if p.r[1] == 0 {
            1
        } else {
            2
        };
        shapes[shape] += 1;
        // A spread of values per component including both range ends, plus
        // the C default reference and a non-default one (the chained case).
        let v0 = [SGRPROJ_PRJ_MIN0, -32, 0, 17, SGRPROJ_PRJ_MAX0];
        let v1 = [SGRPROJ_PRJ_MIN1, -3, 31, 60, SGRPROJ_PRJ_MAX1];
        let refs = [
            SgrprojInfo::c_default().xqd,
            [-70, 80],
            [SGRPROJ_PRJ_MAX0, SGRPROJ_PRJ_MIN1],
        ];
        for &r in &refs {
            for &a in &v0 {
                for &b in &v1 {
                    // C asserts xqd[0] == 0 when r[0] == 0; honour it.
                    let xqd = if p.r[0] == 0 { [0, b] } else { [a, b] };
                    let c_bytes = cref::write_sgrproj_filter_bytes(ep as i32, xqd, r);
                    let mut w = AomWriter::new(64);
                    let info = SgrprojInfo { ep: ep as i32, xqd };
                    let mut refinfo = SgrprojInfo { ep: 0, xqd: r };
                    lr::write_sgrproj_filter(&mut w, &info, &mut refinfo);
                    assert_eq!(
                        c_bytes,
                        w.done().to_vec(),
                        "sgrproj filter bytes diverge ep={ep} xqd={xqd:?} ref={r:?}"
                    );
                    // The writer must chain its reference to the value just
                    // coded (C's trailing svt_memcpy).
                    assert_eq!(refinfo, info, "ref chaining ep={ep}");
                    cells += 1;
                }
            }
        }
    }
    assert!(
        cells > 0 && shapes.iter().all(|&n| n > 0),
        "shapes {shapes:?}"
    );
}

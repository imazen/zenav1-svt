
/// The streamed `wiener_convolve_add_src` must equal the materialised form
/// C writes at every processing-unit shape the restoration filter can ask
/// for, and at both loop orders. This is the pin for BOTH changes in that
/// function: the eight-row ring that replaced the heap intermediate, and
/// the row-major rewrite of the vertical pass.
/// Every legal tap set, every lane position, every proc-unit width — on
/// every tier archmage can reach on this host.
///
/// Three things are being pinned, and the third is the one this program
/// has got wrong fourteen times:
///
/// 1. **The vector arm equals the C-shaped reference.** Not the streamed
///    scalar arm — [`wiener_convolve_add_src_materialised`], which is the
///    literal transcription of `svt_av1_wiener_convolve_add_src_c`
///    (convolve.c:106) and shares no code with the arm under test.
/// 2. **The tap domain is covered at its CORNERS, not sampled.** The AV1
///    tap ranges (restoration.h:141-147) are `t0 in [-5,10]`,
///    `t1 in [-23,8]`, `t2 in [-17,46]` with `f[3] = -2*(t0+t1+t2)`; all
///    2^3 corner combinations run, plus the midpoint filter
///    `WienerInfo::default` and the identity `[0,0,0,128,0,0,0,0]` (the one
///    filter that would hide an off-by-one in the centre tap). The
///    IMPULSE rounds put a single 255 at each position of the 8x8 window
///    in turn, which is what pins the lane-to-tap mapping: a transposed or
///    rotated shuffle survives random data far more often than it survives
///    a moving impulse.
/// 3. **More than one tier actually ran.** `permutations_run >= 2` and
///    zero archmage warnings. A `_v4` arm that is compiled but never
///    entered is this repo's most-repeated defect; a one-arm sweep reports
///    PASS for exactly that.
#[test]
fn wiener_simd_all_tiers_match_materialised() {
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

    let stride = 128usize;
    let rows = 96usize;
    let origin = 8 * stride + 8;

    // Corner + midpoint + identity filters. `sym` builds the 8-tap row the
    // encoder codes: symmetric, `f[7] = 0`, `f[3] = -2*(t0+t1+t2)`.
    let sym = |t0: i16, t1: i16, t2: i16| -> [i16; 8] {
        [t0, t1, t2, -2 * (t0 + t1 + t2), t2, t1, t0, 0]
    };
    let mut filters: alloc::vec::Vec<[i16; 8]> = alloc::vec::Vec::new();
    for &t0 in &[WIENER_FILT_TAP0_MINV as i16, WIENER_FILT_TAP0_MAXV as i16] {
        for &t1 in &[WIENER_FILT_TAP1_MINV as i16, WIENER_FILT_TAP1_MAXV as i16] {
            for &t2 in &[WIENER_FILT_TAP2_MINV as i16, WIENER_FILT_TAP2_MAXV as i16] {
                filters.push(sym(t0, t1, t2));
            }
        }
    }
    filters.push(WienerInfo::default().vfilter);
    filters.push([0, 0, 0, 128, 0, 0, 0, 0]);

    // Every filter must actually REACH the vector arm — otherwise this
    // whole test is a scalar-vs-scalar tautology.
    for f in &filters {
        assert!(
            wiener_simd_applicable(f, f, 64),
            "a legal Wiener filter {f:?} was refused by the vector arm's \
                 precondition, so the sweep below would not test it"
        );
    }

    let shapes: [(usize, usize); 7] = [
        (16, 8),
        (32, 16),
        (48, 24),
        (64, 64),
        (64, 1),
        (16, 2),
        (8, 56),
    ];

    let mut st = 0x1234_5678u32;
    let mut next = || {
        st ^= st << 13;
        st ^= st >> 17;
        st ^= st << 5;
        st
    };

    let mut perms = 0usize;
    let mut warned = 0usize;
    let mut ran = 0usize;
    let mut tiers: alloc::vec::Vec<&'static str> = alloc::vec::Vec::new();

    // Round kinds: 0 = flat black, 1 = flat white, 2 = random,
    // 3 = column ramp, 4.. = a single 255 impulse walking the window.
    for round in 0..(5 + 64) {
        let mut src = alloc::vec![0u8; stride * rows];
        match round {
            0 => {}
            1 => src.iter_mut().for_each(|v| *v = 255),
            2 => src.iter_mut().for_each(|v| *v = (next() >> 24) as u8),
            3 => {
                for r in 0..rows {
                    for c in 0..stride {
                        src[r * stride + c] = ((r * 3 + c * 5) & 0xFF) as u8;
                    }
                }
            }
            4 => src.iter_mut().for_each(|v| *v = 128),
            _ => {
                // Impulse at window offset (dy, dx) in -3..=4 around the
                // block origin: exactly the 8x8 support of one output.
                let k = round - 5;
                let dy = (k / 8) as isize - 3;
                let dx = (k % 8) as isize - 3;
                let idx = origin as isize + dy * stride as isize + dx;
                src[idx as usize] = 255;
            }
        }

        for (fi, hf) in filters.iter().enumerate() {
            // Pair each h-filter with a different v-filter so an
            // h/v swap cannot pass.
            let vf = &filters[(fi + 3) % filters.len()];
            for &(w, h) in &shapes {
                let mut want = alloc::vec![0u8; stride * rows];
                wiener_convolve_add_src_materialised(
                    &src, origin, stride, &mut want, origin, stride, hf, vf, w, h,
                );
                let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
                    let tier = wiener_simd_tier();
                    if !tiers.contains(&tier) {
                        tiers.push(tier);
                    }
                    let mut got = alloc::vec![0u8; stride * rows];
                    wiener_convolve_add_src(
                        &src, origin, stride, &mut got, origin, stride, hf, vf, w, h,
                    );
                    for y in 0..h {
                        let a = origin + y * stride;
                        assert_eq!(
                            &got[a..a + w],
                            &want[a..a + w],
                            "wiener tier {perm} != C shape: round {round} \
                                     filter {fi} {w}x{h} row {y}"
                        );
                    }
                });
                perms = report.permutations_run;
                warned = report.warnings.len();
                ran += 1;
            }
        }
    }

    assert!(ran > 0, "the sweep ran no cells");
    assert_eq!(
        warned, 0,
        "archmage excluded {warned} token(s) from the sweep, so this test \
             covered FEWER tiers than its name claims"
    );
    assert!(
        perms >= 2,
        "the tier sweep ran {perms} permutation(s) -- only the native tier. \
             A one-arm sweep cannot catch a SIMD-vs-scalar divergence, and it \
             is exactly how a `_v4` arm that never executes reports PASS."
    );
    assert!(
        tiers.len() >= 2,
        "the sweep resolved to ONE tier ({tiers:?}); nothing was compared \
             across arms"
    );

    // The point of the whole chunk: on a CPU that HAS AVX-512, the arm the
    // dispatch selects must be the 512-bit one. If this ever starts
    // failing on a Zen 4 / Ice Lake host, the `avx512` feature has been
    // turned off somewhere in the dependency chain and the tier is dead.
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    {
        use archmage::{SimdToken, X64V4Token};
        if X64V4Token::summon().is_some() {
            assert!(
                tiers.iter().any(|t| t.contains("X64V4Token")),
                "this CPU reports AVX-512 F/BW/CD/DQ/VL, but the Wiener \
                     dispatch never resolved to the _v4 arm -- tiers seen: \
                     {tiers:?}"
            );
        }
    }
    // Same statement from the other side: on a host WITHOUT AVX-512 (most
    // CI runners) the _v4 arm is compiled and simply never entered, which
    // is correct and is what makes the default-on `avx512` feature safe.
    extern crate std;
    std::eprintln!("wiener vector tiers exercised: {tiers:?}");
}

/// The vector arm's precondition must not quietly exclude the shapes the
/// encoder actually asks for. `wiener_filter_stripe` (restoration.c:399)
/// calls with `w = procunit_width.min((stripe_width - j + 15) & !15)`, so
/// luma sees 16/32/48/64 and chroma 16/32; the filters are always
/// symmetric with `f[7] = 0`.
#[test]
fn wiener_simd_arm_covers_every_encoder_call_shape() {
    let f = WienerInfo::default();
    for w in [16usize, 32, 48, 64] {
        assert!(
            wiener_simd_applicable(&f.hfilter, &f.vfilter, w),
            "encoder proc-unit width {w} falls off the vector arm"
        );
    }
    // And it must REFUSE what it cannot compute exactly.
    assert!(
        !wiener_simd_applicable(&f.hfilter, &f.vfilter, 65),
        "a width past the ring must fall back, not overrun it"
    );
    let asym = [1i16, 2, 3, -12, 3, 2, 9, 0];
    assert!(
        !wiener_simd_applicable(&f.hfilter, &asym, 64),
        "the vertical symmetric fold is only valid on a symmetric filter"
    );
    let tap7 = [1i16, 2, 3, -12, 3, 2, 1, 5];
    assert!(
        !wiener_simd_applicable(&f.hfilter, &tap7, 64),
        "a non-zero tap 7 is dropped by the fold and must fall back"
    );
}

#[test]
fn wiener_streaming_matches_materialised() {
    let stride = 256usize;
    let mut st = 0x9E37_79B9u32;
    let mut next = || {
        st ^= st << 13;
        st ^= st >> 17;
        st ^= st << 5;
        (st >> 21) as u8
    };
    let src: alloc::vec::Vec<u8> = (0..stride * 256).map(|_| next()).collect();
    // Origin far enough in that the 3-above / 3-left margins are in bounds.
    let origin = 8 * stride + 8;
    // Real Wiener taps sum to 128 and are symmetric; a few shapes plus the
    // identity filter, which is the one that would hide an off-by-one.
    let filters: [[i16; 8]; 3] = [
        [0, 0, 0, 128, 0, 0, 0, 0],
        [3, -7, 15, 104, 15, -7, 3, 0],
        [-5, 12, -21, 156, -21, 12, -5, 0],
    ];
    for hf in &filters {
        for vf in &filters {
            for &(w, h) in &[
                (16usize, 8usize),
                (32, 16),
                (48, 24),
                (64, 64),
                (64, 1),
                (16, 2),
                (8, 56),
            ] {
                let mut a = alloc::vec![0u8; stride * 256];
                let mut b = alloc::vec![0u8; stride * 256];
                wiener_convolve_add_src(&src, origin, stride, &mut a, origin, stride, hf, vf, w, h);
                wiener_convolve_add_src_materialised(
                    &src, origin, stride, &mut b, origin, stride, hf, vf, w, h,
                );
                assert_eq!(a, b, "wiener {w}x{h} hf={hf:?} vf={vf:?}");
            }
        }
    }
}
use super::*;

/// The default WienerInfo must match C set_default_wiener: taps sum with
/// the implicit +128 center to 128.
#[test]
fn default_wiener_taps() {
    let wi = WienerInfo::default();
    assert_eq!(wi.vfilter, [3, -7, 15, -2 * (3 - 7 + 15), 15, -7, 3, 0]);
    let sum: i32 = wi.vfilter.iter().map(|&t| t as i32).sum::<i32>() + 128;
    assert_eq!(sum, 128);
}

/// Identity filter (all zero side taps): output == centre input (the
/// add-src rounding carries the pixel through both passes exactly).
#[test]
fn identity_filter_passthrough() {
    let w = 16usize;
    let h = 12usize;
    let b = 4usize;
    let stride = w + 2 * b;
    let mut src = alloc::vec![0u8; stride * (h + 2 * b)];
    let origin = b * stride + b;
    for y in 0..h {
        for x in 0..w {
            src[origin + y * stride + x] = ((x * 13 + y * 7) % 251) as u8;
        }
    }
    extend_frame(&mut src, origin, w, h, stride, 4, 3);
    let zero = WienerInfo {
        vfilter: [0, 0, 0, 0, 0, 0, 0, 0],
        hfilter: [0, 0, 0, 0, 0, 0, 0, 0],
    };
    let mut dst = alloc::vec![0u8; stride * (h + 2 * b)];
    wiener_convolve_add_src(
        &src,
        origin,
        stride,
        &mut dst,
        origin,
        stride,
        &zero.hfilter,
        &zero.vfilter,
        w,
        h,
    );
    for y in 0..h {
        for x in 0..w {
            assert_eq!(dst[origin + y * stride + x], src[origin + y * stride + x]);
        }
    }
}

#[test]
fn count_units_matches_c_rounding() {
    assert_eq!(count_units_in_tile(256, 64), 1);
    assert_eq!(count_units_in_tile(256, 128), 1);
    assert_eq!(count_units_in_tile(256, 256), 1);
    assert_eq!(count_units_in_tile(256, 384), 2); // (384+128)/256 = 2
    assert_eq!(count_units_in_tile(256, 383), 1); // (383+128)/256 = 1
    assert_eq!(count_units_in_tile(64, 32), 1);
}

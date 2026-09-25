use super::*;

/// `get_conv_params_no_round`'s documented arms, checked against the C
/// constants: single prediction is 3/11, compound is 3/7, and the
/// `intbufrange` correction fires only at bd 12.
#[test]
fn conv_params_rounds() {
    let single = ConvolveParams::single(false, 8);
    assert_eq!((single.round_0, single.round_1), (3, 11));
    let compound = ConvolveParams::no_round(false, 64, true, 8);
    assert_eq!((compound.round_0, compound.round_1), (3, 7));
    let bd10 = ConvolveParams::single(false, 10);
    assert_eq!((bd10.round_0, bd10.round_1), (3, 11));
    // bd 12: intbufrange = 12 + 7 - 3 + 2 = 18 > 16, so +2 / -2.
    let bd12 = ConvolveParams::single(false, 12);
    assert_eq!((bd12.round_0, bd12.round_1), (5, 9));
    let bd12c = ConvolveParams::no_round(false, 64, true, 12);
    assert_eq!((bd12c.round_0, bd12c.round_1), (5, 7));
}

/// The narrow-block substitution maps SHARP onto the *regular* 4-tap
/// table, which is the arm a "sharp -> sharp" assumption gets wrong.
#[test]
fn narrow_block_filter_substitution() {
    let sharp4 = interp_filter_params_with_block_size(InterpFilterKind::MultiTapSharp, 4);
    assert_eq!(sharp4.kernels[1], SUB_PEL_FILTERS_4[1]);
    let sharp8 = interp_filter_params_with_block_size(InterpFilterKind::MultiTapSharp, 8);
    assert_eq!(sharp8.kernels[1], SUB_PEL_FILTERS_8SHARP[1]);
    let smooth4 = interp_filter_params_with_block_size(InterpFilterKind::EightTapSmooth, 4);
    assert_eq!(smooth4.kernels[1], SUB_PEL_FILTERS_4SMOOTH[1]);
    // BILINEAR is never substituted.
    let bil4 = interp_filter_params_with_block_size(InterpFilterKind::Bilinear, 4);
    assert_eq!(bil4.kernels[1], BILINEAR_FILTERS[1]);
}

/// Both 4-tap tables normalize to 128 like the 8-tap ones.
#[test]
fn four_tap_tables_sum_to_128() {
    for (name, t) in [
        ("sub_pel_filters_4", &SUB_PEL_FILTERS_4),
        ("sub_pel_filters_4smooth", &SUB_PEL_FILTERS_4SMOOTH),
    ] {
        for (phase, k) in t.iter().enumerate() {
            let s: i32 = k.iter().map(|&v| v as i32).sum();
            assert_eq!(s, 128, "{name} phase {phase} sums to {s}");
        }
    }
}

/// Positive witness that the jnt `_v4` arms are EXERCISED and exact, not
/// just compiled — the c_parity tests pass either way since the scalar
/// fallback is also correct. The oracle is the scalar body transcribed
/// per kernel; each `w` covers the 32-px rung, the 16-px rung, and the
/// scalar tail (w = 56 -> 32 + 16 + 8).
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[test]
fn jnt_v4_arms_match_scalar_when_summoned() {
    extern crate std;
    use alloc::vec;
    use alloc::vec::Vec;
    let Some(v4) = X64V4Token::summon() else {
        std::eprintln!("X64V4Token unavailable — jnt _v4 arms NOT exercised on this host");
        return;
    };
    let fp = interp_filter_params_with_block_size(InterpFilterKind::EightTapRegular, 32);
    let mk_cp = |do_average: bool, use_jnt: bool| {
        let mut cp = ConvolveParams::no_round(do_average, 64, true, 8);
        cp.use_jnt_comp_avg = use_jnt;
        cp.fwd_offset = 9;
        cp.bck_offset = 7;
        cp
    };
    let stride = 80usize;
    let src: Vec<u8> = (0..stride * 80)
        .map(|i| ((i * 37 + 11) % 251) as u8)
        .collect();
    let view = SrcView::new(&src, 8 * stride + 8, stride);
    let bd = 8i32;

    for &(w, h) in &[(16usize, 8usize), (32, 8), (56, 8), (64, 16)] {
        for cp in [mk_cp(false, false), mk_cp(true, false), mk_cp(true, true)] {
            let cb_stride = cp.dst_stride;
            // --- scalar oracles (verbatim transcriptions) ---
            let off = bd + 2 * FILTER_BITS - cp.round_0;
            let roff = (1 << (off - cp.round_1)) + (1 << (off - cp.round_1 - 1));
            let rbits = 2 * FILTER_BITS - cp.round_0 - cp.round_1;
            let mut cb0 = vec![0u16; cb_stride * h];
            let mut d0 = vec![0u8; w * h];
            let mut cb1 = cb0.clone();
            let mut d1 = d0.clone();

            // jnt_convolve_x
            let xf = fp.subpel_kernel(5);
            let bits = FILTER_BITS - cp.round_1;
            for y in 0..h {
                for x in 0..w {
                    let mut res = 0i32;
                    for k in 0..SUBPEL_TAPS {
                        res += xf[k] as i32 * view.at(y as i32, x as i32 - 3 + k as i32);
                    }
                    let res = (1 << bits) * round_power_of_two(res, cp.round_0) + roff;
                    if cp.do_average {
                        d0[y * w + x] = jnt_average(cb0[y * cb_stride + x], res, roff, rbits, &cp);
                    } else {
                        cb0[y * cb_stride + x] = res as u16;
                    }
                }
            }
            jnt_convolve_x_v4(v4, view, &mut d1, w, &mut cb1, w, h, xf, &cp);
            assert_eq!(cb0, cb1, "jnt_x_v4 conv_buf {w}x{h} avg={}", cp.do_average);
            assert_eq!(d0, d1, "jnt_x_v4 dst {w}x{h} avg={}", cp.do_average);

            // jnt_convolve_y
            let yf = fp.subpel_kernel(9);
            let bits = FILTER_BITS - cp.round_0;
            let mut cb0 = vec![0u16; cb_stride * h];
            let mut d0 = vec![0u8; w * h];
            let mut cb1 = cb0.clone();
            let mut d1 = d0.clone();
            for y in 0..h {
                for x in 0..w {
                    let mut res = 0i32;
                    for k in 0..SUBPEL_TAPS {
                        res += yf[k] as i32 * view.at(y as i32 - 3 + k as i32, x as i32);
                    }
                    let res = round_power_of_two(res * (1 << bits), cp.round_1) + roff;
                    if cp.do_average {
                        d0[y * w + x] = jnt_average(cb0[y * cb_stride + x], res, roff, rbits, &cp);
                    } else {
                        cb0[y * cb_stride + x] = res as u16;
                    }
                }
            }
            jnt_convolve_y_v4(v4, view, &mut d1, w, &mut cb1, w, h, yf, &cp);
            assert_eq!(cb0, cb1, "jnt_y_v4 conv_buf {w}x{h} avg={}", cp.do_average);
            assert_eq!(d0, d1, "jnt_y_v4 dst {w}x{h} avg={}", cp.do_average);

            // jnt_convolve_2d
            let mut cb0 = vec![0u16; cb_stride * h];
            let mut d0 = vec![0u8; w * h];
            let mut cb1 = cb0.clone();
            let mut d1 = d0.clone();
            let im_h = h + SUBPEL_TAPS - 1;
            let mut im = vec![0i16; im_h * w];
            for y in 0..im_h {
                for x in 0..w {
                    let mut sum = 1i32 << (bd + FILTER_BITS - 1);
                    for k in 0..SUBPEL_TAPS {
                        sum += xf[k] as i32 * view.at(y as i32 - 3, x as i32 - 3 + k as i32);
                    }
                    im[y * w + x] = round_power_of_two(sum, cp.round_0) as i16;
                }
            }
            for y in 0..h {
                for x in 0..w {
                    let mut sum = 1i32 << off;
                    for k in 0..SUBPEL_TAPS {
                        sum += yf[k] as i32 * im[(y + k) * w + x] as i32;
                    }
                    let res = round_power_of_two(sum, cp.round_1) as u16;
                    if cp.do_average {
                        d0[y * w + x] =
                            jnt_average(cb0[y * cb_stride + x], res as i32, roff, rbits, &cp);
                    } else {
                        cb0[y * cb_stride + x] = res;
                    }
                }
            }
            jnt_convolve_2d_v4(v4, view, &mut d1, w, &mut cb1, w, h, xf, yf, &cp);
            assert_eq!(cb0, cb1, "jnt_2d_v4 conv_buf {w}x{h} avg={}", cp.do_average);
            assert_eq!(d0, d1, "jnt_2d_v4 dst {w}x{h} avg={}", cp.do_average);

            // jnt_convolve_2d_copy
            let mut cb0 = vec![0u16; cb_stride * h];
            let mut d0 = vec![0u8; w * h];
            let mut cb1 = cb0.clone();
            let mut d1 = d0.clone();
            let bits = FILTER_BITS * 2 - cp.round_1 - cp.round_0;
            for y in 0..h {
                for x in 0..w {
                    let res =
                        ((view.at(y as i32, x as i32) as u16) << bits).wrapping_add(roff as u16);
                    if cp.do_average {
                        d0[y * w + x] =
                            jnt_average(cb0[y * cb_stride + x], res as i32, roff, rbits, &cp);
                    } else {
                        cb0[y * cb_stride + x] = res;
                    }
                }
            }
            jnt_convolve_2d_copy_v4(v4, view, &mut d1, w, &mut cb1, w, h, &cp);
            assert_eq!(
                cb0, cb1,
                "jnt_copy_v4 conv_buf {w}x{h} avg={}",
                cp.do_average
            );
            assert_eq!(d0, d1, "jnt_copy_v4 dst {w}x{h} avg={}", cp.do_average);
        }
    }
}

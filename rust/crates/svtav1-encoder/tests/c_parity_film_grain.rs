//! Exported pinned-C oracles for the complete grain translation. Integer output
//! and floating intermediates compare exactly, including float bit patterns.
use svtav1_cref::film_grain as c;
use svtav1_encoder::entropy::obu::FilmGrainParams;
use svtav1_encoder::{
    film_grain_denoise as denoise, film_grain_fft as fft, film_grain_model as model,
    film_grain_synthesis as synthesis,
};
fn cgrain(p: &FilmGrainParams, depth: u8) -> c::Grain {
    c::Grain {
        apply_grain: p.apply_grain as i32,
        update_parameters: p.apply_grain as i32,
        bit_depth: depth as i32,
        random_seed: p.random_seed,
        num_y_points: p.num_y_points as i32,
        num_cb_points: p.num_cb_points as i32,
        num_cr_points: p.num_cr_points as i32,
        scaling_points_y: p.scaling_points_y.map(|v| v.map(i32::from)),
        scaling_points_cb: p.scaling_points_cb.map(|v| v.map(i32::from)),
        scaling_points_cr: p.scaling_points_cr.map(|v| v.map(i32::from)),
        ar_coeffs_y: p.ar_coeffs_y.map(i32::from),
        ar_coeffs_cb: p.ar_coeffs_cb.map(i32::from),
        ar_coeffs_cr: p.ar_coeffs_cr.map(i32::from),
        scaling_shift: p.scaling_shift as i32,
        ar_coeff_lag: p.ar_coeff_lag as i32,
        ar_coeff_shift: p.ar_coeff_shift as i32,
        grain_scale_shift: p.grain_scale_shift as i32,
        cb_mult: p.cb_mult as i32,
        cb_luma_mult: p.cb_luma_mult as i32,
        cb_offset: p.cb_offset as i32,
        cr_mult: p.cr_mult as i32,
        cr_luma_mult: p.cr_luma_mult as i32,
        cr_offset: p.cr_offset as i32,
        overlap_flag: p.overlap_flag as i32,
        clip_to_restricted_range: p.clip_to_restricted_range as i32,
        chroma_scaling_from_luma: p.chroma_scaling_from_luma as i32,
        ignore_ref: 0,
    }
}
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 as u32
    }
}
fn planes(
    w: usize,
    h: usize,
    strides: [usize; 3],
    depth: u8,
    seed: u64,
    kind: usize,
) -> [Vec<u16>; 3] {
    let mut rng = Rng(seed);
    core::array::from_fn(|c| {
        let sub = usize::from(c > 0);
        let pw = w.div_ceil(1 << sub);
        let ph = h.div_ceil(1 << sub);
        let mut p = vec![0; strides[c] * ph];
        for y in 0..ph {
            for x in 0..pw {
                let noise = (rng.next() % 25) as i32 - 12;
                let sample = match kind {
                    0 => 128,
                    1 => 100 + noise,
                    2 => 32 + (x * 173 / pw) as i32 + noise,
                    _ => (rng.next() % 256) as i32,
                };
                p[y * strides[c] + x] = ((sample.clamp(0, 255) << (depth - 8)) as u16
                    + if depth == 10 {
                        (rng.next() % 4) as u16
                    } else {
                        0
                    })
                .min((1 << depth) - 1);
            }
        }
        p
    })
}
fn eqplanes(a: &[Vec<u16>; 3], b: &[Vec<u16>; 3], label: &str) {
    for c in 0..3 {
        assert_eq!(a[c].len(), b[c].len());
        for i in 0..a[c].len() {
            assert_eq!(a[c][i], b[c][i], "{label} plane={c} sample={i}");
        }
    }
}
#[test]
fn fft_inverse_and_filter_match_exported_c() {
    let mut rng = Rng(7391);
    for n in [2, 4, 8, 16, 32] {
        for case in 0..8 {
            let input: Vec<_> = (0..n * n)
                .map(|i| {
                    if case == 0 {
                        1.0
                    } else if case == 1 {
                        f32::from(i == 0)
                    } else {
                        (rng.next() % 20001) as f32 / 10000.0 - 1.0
                    }
                })
                .collect();
            let mut r = vec![0.0; 2 * n * n];
            let mut ct = r.clone();
            let mut tmp = r.clone();
            let mut ctmp = r.clone();
            fft::forward(&input, &mut tmp, &mut r, n);
            c::fft(n, false, &input, &mut ctmp, &mut ct);
            for i in 0..r.len() {
                assert_eq!(
                    r[i].to_bits(),
                    ct[i].to_bits(),
                    "forward n={n} case={case} i={i}"
                );
            }
            fft::filter(&mut r, 0.0003);
            c::filter(n, &mut ct, 0.0003);
            for i in 0..r.len() {
                assert_eq!(
                    r[i].to_bits(),
                    ct[i].to_bits(),
                    "filter n={n} case={case} i={i}"
                );
            }
            let mut ir = vec![0.0; 2 * n * n];
            let mut ic = ir.clone();
            fft::inverse(&r, &mut tmp, &mut ir, n);
            c::fft(n, true, &ct, &mut ctmp, &mut ic);
            for i in 0..n * n {
                assert_eq!(
                    ir[i].to_bits(),
                    ic[i].to_bits(),
                    "inverse n={n} case={case} i={i}"
                );
            }
        }
    }
}
#[test]
fn flat_block_finder_and_extraction_match_c() {
    for depth in [8, 10] {
        for bs in [4, 8, 16, 32] {
            for kind in 0..4 {
                let (w, h, stride) = (70, 66, 80);
                let src = planes(w, h, [stride, 40, 40], depth, 8273, kind);
                let finder = model::FlatBlockFinder::new(bs, depth);
                let (cf, cp, cb) = c::flat(&src[0], w, h, stride, bs, depth, -3, 61);
                let mut rp = vec![0.0; bs * bs];
                let mut rb = rp.clone();
                finder.extract(&src[0], w, h, stride, -3, 61, &mut rp, &mut rb);
                for i in 0..rp.len() {
                    assert_eq!(
                        rp[i].to_bits(),
                        cp[i].to_bits(),
                        "plane depth={depth} bs={bs} kind={kind} i={i}"
                    );
                    assert_eq!(
                        rb[i].to_bits(),
                        cb[i].to_bits(),
                        "block depth={depth} bs={bs} kind={kind} i={i}"
                    );
                }
                assert_eq!(
                    finder.run(&src[0], w, h, stride),
                    cf,
                    "flat depth={depth} bs={bs} kind={kind}"
                );
            }
        }
    }
}
#[test]
fn strength_solve_and_piecewise_fit_match_c() {
    for depth in [8, 10] {
        for points in [2, 10, 14, 20] {
            for kind in 0..4 {
                let mut s =
                    svtav1_encoder::port_noise_model::NoiseStrengthSolver::new(20, depth as u32);
                let means: Vec<_> = (0..80)
                    .map(|i| i as f64 * ((1 << depth) - 1) as f64 / 79.0)
                    .collect();
                let stds: Vec<_> = (0..80)
                    .map(|i| match kind {
                        0 => 2.5,
                        1 => (i % 9) as f64,
                        2 => i as f64 / 9.0,
                        _ => ((i * 17) % 23) as f64 / 3.0,
                    })
                    .collect();
                for (&mean, &std) in means.iter().zip(&stds) {
                    s.add_measurement(mean, std);
                }
                assert!(s.solve());
                let (a, b, x, lut) = c::solver(20, depth, &means, &stds, points);
                for (label, r, c) in [("a", &s.a, &a), ("b", &s.b, &b), ("x", &s.x, &x)] {
                    for i in 0..r.len() {
                        assert_eq!(
                            r[i].to_bits(),
                            c[i].to_bits(),
                            "{label} depth={depth} kind={kind} i={i}"
                        );
                    }
                }
                assert_eq!(
                    s.fit_piecewise(points),
                    lut,
                    "lut depth={depth} kind={kind} points={points}"
                );
            }
        }
    }
}
#[test]
fn wiener_pixels_match_c() {
    for depth in [8, 10] {
        for bs in [8, 16, 32] {
            for kind in 0..4 {
                let (w, h, strides) = (70, 66, [80, 40, 40]);
                let source = planes(w, h, strides, depth, 98234, kind);
                let data = source.each_ref().map(Vec::as_slice);
                let psd = [0.0007, 0.0002, 0.0002];
                let r = denoise::wiener_denoise(data, w, h, strides, depth, bs, psd);
                let c = c::wiener(data, w, h, strides, depth, bs, psd);
                eqplanes(&r, &c, &format!("wiener depth={depth} bs={bs} kind={kind}"));
            }
        }
    }
}
#[test]
fn denoise_model_parameters_match_c() {
    let mut live = 0;
    for depth in [8, 10] {
        for adaptive in [false, true] {
            for strength in [1, 25, 50] {
                for kind in 0..4 {
                    let (w, h, strides) = (96, 80, [104, 52, 52]);
                    let source = planes(w, h, strides, depth, 92834, kind);
                    let data = source.each_ref().map(Vec::as_slice);
                    let (r, rp, _) = denoise::denoise_and_model(
                        data, w, h, strides, depth, strength, adaptive, 7391,
                    );
                    let (c, cp) = c::model(data, w, h, strides, depth, strength, adaptive, 7391);
                    let label = format!(
                        "model depth={depth} adaptive={adaptive} strength={strength} kind={kind}"
                    );
                    eqplanes(&r, &c, &label);
                    assert_eq!(cgrain(&rp, 0), cp, "{label}");
                    live += usize::from(rp.apply_grain);
                }
            }
        }
    }
    assert!(live > 12, "insufficient successful model fits: {live}");
}
#[test]
fn synthesis_all_modes_match_c() {
    let mut rng = Rng(48237);
    for depth in [8, 10] {
        for lag in 0..=3 {
            for overlap in [false, true] {
                for mode in 0..4 {
                    for (w, h) in [
                        (2usize, 2usize),
                        (30, 18),
                        (32, 32),
                        (34, 66),
                        (70, 82),
                        (71, 83),
                    ] {
                        let mut p = FilmGrainParams {
                            apply_grain: true,
                            random_seed: rng.next() as u16,
                            scaling_shift: 8 + (rng.next() % 4) as u8,
                            ar_coeff_lag: lag,
                            ar_coeff_shift: 6 + (rng.next() % 4) as u8,
                            grain_scale_shift: (rng.next() % 4) as u8,
                            overlap_flag: overlap,
                            clip_to_restricted_range: mode % 2 == 1,
                            chroma_scaling_from_luma: mode == 2,
                            num_y_points: if mode == 3 { 0 } else { 4 },
                            num_cb_points: if mode == 2 { 0 } else { 3 },
                            num_cr_points: if mode == 2 { 0 } else { 3 },
                            cb_mult: 127,
                            cb_luma_mult: 191,
                            cb_offset: 254,
                            cr_mult: 177,
                            cr_luma_mult: 121,
                            cr_offset: 272,
                            ..Default::default()
                        };
                        p.scaling_points_y[..4].copy_from_slice(&[
                            [0, 11],
                            [37, 170],
                            [181, 83],
                            [255, 1],
                        ]);
                        p.scaling_points_cb[..3].copy_from_slice(&[
                            [19, 90],
                            [128, 201],
                            [240, 43],
                        ]);
                        p.scaling_points_cr[..3].copy_from_slice(&[[0, 44], [80, 192], [255, 35]]);
                        for coeff in p
                            .ar_coeffs_y
                            .iter_mut()
                            .chain(&mut p.ar_coeffs_cb)
                            .chain(&mut p.ar_coeffs_cr)
                        {
                            *coeff = (rng.next() % 31) as i16 - 15;
                        }
                        let strides = [w + 9, w.div_ceil(2) + 5, w.div_ceil(2) + 5];
                        let mut r = planes(w, h, strides, depth, 29735, 3);
                        let mut c = r.clone();
                        let [y, u, v] = &mut r;
                        synthesis::add_grain(&p, [y, u, v], strides, w, h, depth);
                        let [y, u, v] = &mut c;
                        c::synthesis(&cgrain(&p, depth), [y, u, v], w, h, strides, depth);
                        eqplanes(
                            &r,
                            &c,
                            &format!(
                                "synthesis depth={depth} lag={lag} overlap={overlap} mode={mode} {w}x{h}"
                            ),
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn model_stream_witness_matches_c() {
    let (w, h, strides) = (128usize, 128usize, [128, 64, 64]);
    let mut source = [
        vec![0u16; w * h],
        vec![128; w * h / 4],
        vec![128; w * h / 4],
    ];
    for r in 0..h {
        for c in 0..w {
            let mut z = (r * w + c + 1) as u32;
            z ^= z << 13;
            z ^= z >> 17;
            z ^= z << 5;
            source[0][r * w + c] = (80 + r * 80 / h + z as usize % 25) as u16;
        }
    }
    let data = source.each_ref().map(Vec::as_slice);
    let (r, rp, _) = denoise::denoise_and_model(data, w, h, strides, 8, 25, true, 7391);
    let (c, cp) = c::model(data, w, h, strides, 8, 25, true, 7391);
    eqplanes(&r, &c, "stream witness");
    assert_eq!(cgrain(&rp, 0), cp);
}

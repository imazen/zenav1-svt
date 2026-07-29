//! Per-kernel NEON-vs-forced-scalar for the DSP inner loops.
//!
//! SAD and SATD run at every candidate position in motion/mode search, and
//! CDEF's direction search runs per 8x8 block — they dominate encode time.
//! Nothing here compares a kernel against its OWN scalar fallback, so a NEON
//! path slower than the autovectorized scalar one would be invisible. That
//! failure mode was real elsewhere in the 2026-07-28 aarch64 sweep: three
//! zenfilters kernels and zenzstd's count_match all lost to their scalar
//! tiers, and count_match lost specifically because it was an x86
//! movemask-shaped algorithm inherited by NEON — a risk this crate shares,
//! being a port of C code written against x86 intrinsics.
//!
//! NOTE: on aarch64 NEON is BASELINE, so the "scalar" arm is autovectorized
//! too. ~1.00x means both compiled to equivalent work; BELOW 1.00 is the bug.
//!
//! Run: `cargo bench -p zenav1-svt-dsp --bench kernel_tiers`

use svtav1_dsp::{cdef, fwd_txfm, hadamard, hbd, intra_pred, quant_coding, restoration, inv_txfm, quant, sad, variance};
use zenbench::prelude::*;

#[cfg(target_arch = "aarch64")]
type TierToken = archmage::NeonToken;
#[cfg(target_arch = "x86_64")]
type TierToken = archmage::X64V3Token;

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
const TIER_NAME: &str = if cfg!(target_arch = "aarch64") { "neon" } else { "v3(avx2)" };

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
fn set_simd(enabled: bool) -> bool {
    use archmage::SimdToken;
    TierToken::dangerously_disable_token_process_wide(!enabled).is_ok()
}
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn set_simd(_e: bool) -> bool { false }

fn plane8(n: usize, seed: u32) -> Vec<u8> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 24) as u8
        })
        .collect()
}

fn bench_dsp(suite: &mut Suite) {
    if !set_simd(true) || !set_simd(false) {
        eprintln!("[kernel_tiers] SIMD tier not toggleable here. Skipping.");
        return;
    }
    set_simd(true);
    eprintln!("[kernel_tiers] comparing {TIER_NAME} vs forced scalar");

    // 64-wide planes so every block size below has stride room.
    const STRIDE: usize = 128;
    let src: &'static [u8] = Box::leak(plane8(STRIDE * 128, 3).into_boxed_slice());
    let rf: &'static [u8] = Box::leak(plane8(STRIDE * 128, 7).into_boxed_slice());

    macro_rules! pair {
        ($name:expr, $call:expr) => {
            suite.compare($name, |g| {
                for (arm, simd) in [(TIER_NAME, true), ("scalar", false)] {
                    g.bench(arm, move |b| {
                        b.iter(move || {
                            set_simd(simd);
                            $call
                        })
                    });
                }
            });
        };
    }

    pair!("sad_8x8", sad::sad_8x8(src, STRIDE, rf, STRIDE));
    pair!("sad_16x16", sad::sad_16x16(src, STRIDE, rf, STRIDE));
    pair!("sad_32x32", sad::sad_32x32(src, STRIDE, rf, STRIDE));
    pair!("sad_64x64", sad::sad_64x64(src, STRIDE, rf, STRIDE));
    pair!("satd_4x4", hadamard::satd_4x4(src, STRIDE, rf, STRIDE));
    pair!("satd_8x8", hadamard::satd_8x8(src, STRIDE, rf, STRIDE));
    pair!("cdef_find_dir_8bit", cdef::cdef_find_dir_8bit(src, STRIDE, 0));
    {
        // Same input offset the C-parity tests use.
        const CDEF_IOFF: usize = cdef::CDEF_BSTRIDE * cdef::CDEF_VBORDER + cdef::CDEF_HBORDER;
        // CDEF filter on the 8x8 shape that takes the vector path.
        let inb: &'static [u16] = Box::leak(
            (0..cdef::CDEF_INBUF_SIZE)
                .map(|i| ((i * 7919) % 1024) as u16)
                .collect::<Vec<u16>>()
                .into_boxed_slice(),
        );
        suite.compare("cdef_filter_block_8x8", |g| {
            for (arm, simd) in [(TIER_NAME, true), ("scalar", false)] {
                g.bench(arm, move |b| {
                    let mut dst = vec![0u8; 64];
                    b.iter(move || {
                        set_simd(simd);
                        cdef::cdef_filter_block(
                            &mut dst, 0, 8, inb, CDEF_IOFF, 12, 2, 1, 6, 6,
                            cdef::BLOCK_8X8, 0, 1,
                        );
                    })
                });
            }
        });
    }
    pair!("variance_16x16", variance::variance(src, STRIDE, 16, 16));
    pair!("variance_64x64", variance::variance(src, STRIDE, 64, 64));
    pair!("sse_16x16", variance::sse(src, STRIDE, rf, STRIDE, 16, 16));
    pair!("sse_64x64", variance::sse(src, STRIDE, rf, STRIDE, 64, 64));

    // Intra prediction is THE hot path in an all-intra (AVIF) encoder.
    {
        let above: &'static [u8] = Box::leak(plane8(64, 11).into_boxed_slice());
        let left: &'static [u8] = Box::leak(plane8(64, 13).into_boxed_slice());
        for &(label, w, h) in &[("16x16", 16usize, 16usize), ("32x32", 32, 32)] {
            suite.compare(format!("paeth_{label}"), |g| {
                for (arm, simd) in [(TIER_NAME, true), ("scalar", false)] {
                    g.bench(arm, move |b| {
                        let mut dst = vec![0u8; STRIDE * h];
                        b.iter(move || {
                            set_simd(simd);
                            intra_pred::predict_paeth(&mut dst, STRIDE, above, left, 128, w, h);
                        })
                    });
                }
            });
        }
    }

    {
        // HBD (dst16) CDEF: same column kernel as the 8-bit arm, u16 output.
        let inb: &'static [u16] = Box::leak(
            (0..cdef::CDEF_INBUF_SIZE).map(|i| ((i * 7919) % 4096) as u16)
                .collect::<Vec<u16>>().into_boxed_slice(),
        );
        const IOFF2: usize = cdef::CDEF_BSTRIDE * cdef::CDEF_VBORDER + cdef::CDEF_HBORDER;
        suite.compare("cdef_filter_block_hbd_8x8", |g| {
            for (arm, simd) in [(TIER_NAME, true), ("scalar", false)] {
                g.bench(arm, move |b| {
                    let mut dst = vec![0u16; 64];
                    b.iter(move || {
                        set_simd(simd);
                        hbd::cdef_filter_block_hbd(
                            &mut dst, 0, 8, inb, IOFF2, 12, 2, 1, 6, 6, cdef::BLOCK_8X8, 0, 1,
                        );
                    })
                });
            }
        });
    }

    {
        // Raster quantizers: whole-block quantize over 1024 coeffs (32x32).
        let coeffs: &'static [i32] = Box::leak(
            (0..1024).map(|i| (((i * 5779) % 4001) as i32) - 2000)
                .collect::<Vec<i32>>().into_boxed_slice(),
        );
        for (name, is_fp) in [("quantize_fp_raster_1024", true), ("quantize_b_raster_1024", false)] {
            suite.compare(name, move |g| {
                for (arm, simd) in [(TIER_NAME, true), ("scalar", false)] {
                    g.bench(arm, move |b| {
                        let mut q = vec![0i32; 1024];
                        let mut dq = vec![0i32; 1024];
                        b.iter(move || {
                            set_simd(simd);
                            if is_fp {
                                quant_coding::quantize_fp_raster(
                                    coeffs, &mut q, &mut dq, &[195, 349], &[125, 70], &[522, 933], 1,
                                );
                            } else {
                                quant_coding::quantize_b_raster(
                                    coeffs, &mut q, &mut dq, &[326, 583], &[195, 349],
                                    &[-1255, -29571], &[128, 128], &[522, 933], 1,
                                );
                            }
                        })
                    });
                }
            });
        }
    }

    {
        // Wiener compute_stats: M/H moments over a 64x64 restoration unit.
        const W: usize = 64; const B: usize = 4; const STR: usize = W + 2 * B;
        let dgd: &'static [u8] = Box::leak(
            (0..STR * (W + 2 * B)).map(|i| ((i * 7919) % 256) as u8)
                .collect::<Vec<u8>>().into_boxed_slice(),
        );
        for win in [5usize, 7] {
            suite.compare(&format!("wiener_compute_stats_win{win}_64x64"), move |g| {
                for (arm, simd) in [(TIER_NAME, true), ("scalar", false)] {
                    g.bench(arm, move |b| {
                        let mut mm = vec![0i64; win * win];
                        let mut hh = vec![0i64; win * win * win * win];
                        b.iter(move || {
                            set_simd(simd);
                            restoration::compute_stats(
                                win, dgd, B * STR + B, STR, dgd, B * STR + B, STR,
                                0, W as i32, 0, W as i32, &mut mm, &mut hh,
                            );
                        })
                    });
                }
            });
        }
    }

    // Quantize: measured to decide whether a magic-reciprocal NEON port is
    // worth the risk. It divides by a loop-invariant `dequant` per
    // coefficient, and NEON has no integer divide.
    {
        let coeffs: &'static [i32] = Box::leak(
            (0..1024).map(|i| ((i * 7919) % 8192) as i32 - 4096).collect::<Vec<i32>>().into_boxed_slice(),
        );
        suite.compare("quantize", |g| {
            for &(label, n) in &[("64_coeffs", 64usize), ("1024_coeffs", 1024)] {
                g.bench(label, move |b| {
                    let qp = quant::QuantParam { dequant: [20, 24], shift: 2 };
                    let mut qc = vec![0i32; n];
                    let mut dqc = vec![0i32; n];
                    b.iter(move || quant::quantize(&coeffs[..n], &qp, &mut qc, &mut dqc, n))
                });
            }
        });
    }

    // Transforms: measured to decide whether a from-scratch bit-exact NEON
    // butterfly is warranted. All three tiers currently call the same
    // `*_c_exact` body, so these are absolute-cost numbers, not a tier
    // comparison.
    {
        let coeffs: &'static [i32] = Box::leak(
            (0..1024).map(|i| ((i * 7919) % 4096) as i32 - 2048).collect::<Vec<i32>>().into_boxed_slice(),
        );
        suite.compare("fwd_txfm2d_dct_dct", |g| {
            g.bench("4x4", move |b| {
                let mut out = vec![0i32; 16];
                b.iter(move || fwd_txfm::fwd_txfm2d_4x4_dct_dct(coeffs, &mut out, 4))
            });
            g.bench("8x8", move |b| {
                let mut out = vec![0i32; 64];
                b.iter(move || fwd_txfm::fwd_txfm2d_8x8_dct_dct(coeffs, &mut out, 8))
            });
            g.bench("16x16", move |b| {
                let mut out = vec![0i32; 256];
                b.iter(move || fwd_txfm::fwd_txfm2d_16x16_dct_dct(coeffs, &mut out, 16))
            });
        });
        suite.compare("inv_txfm2d_dct_dct", |g| {
            g.bench("8x8", move |b| {
                let mut out = vec![0i32; 64];
                b.iter(move || inv_txfm::inv_txfm2d_8x8_dct_dct(coeffs, &mut out, 8))
            });
        });
    }

    set_simd(true);
}

zenbench::main!(bench_dsp);

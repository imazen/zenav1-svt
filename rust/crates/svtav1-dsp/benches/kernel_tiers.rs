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

use svtav1_dsp::{cdef, hadamard, sad};
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

    set_simd(true);
}

zenbench::main!(bench_dsp);

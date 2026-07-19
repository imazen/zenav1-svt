//! [SVT_HDR_MODE] photon-noise table generation differential: the Rust
//! `noise_gen::generate_noise_table` vs the REAL exported C
//! `svt_av1_generate_noise_table` (noise_generation.c), across the
//! strength x chroma x cfl x size x resolution x range grid. The C shim
//! builds a real EbSvtAv1EncConfiguration and flattens the returned
//! AomFilmGrain, so struct layout is the library's own ABI.

use svtav1_cref as cref;
use svtav1_encoder::noise_gen;

/// Flatten the Rust params in the shim's 159-i32 layout.
fn flatten(fg: &noise_gen::FilmGrainParams) -> Vec<i32> {
    let mut o = Vec::with_capacity(159);
    o.push(i32::from(fg.apply_grain));
    o.push(fg.num_y_points as i32);
    for p in &fg.scaling_points_y {
        o.push(i32::from(p[0]));
        o.push(i32::from(p[1]));
    }
    o.push(i32::from(fg.chroma_scaling_from_luma));
    o.push(fg.num_cb_points as i32);
    for p in &fg.scaling_points_cb {
        o.push(i32::from(p[0]));
        o.push(i32::from(p[1]));
    }
    o.push(fg.num_cr_points as i32);
    for p in &fg.scaling_points_cr {
        o.push(i32::from(p[0]));
        o.push(i32::from(p[1]));
    }
    o.push(i32::from(fg.scaling_shift));
    o.push(i32::from(fg.ar_coeff_lag));
    for &c in &fg.ar_coeffs_y {
        o.push(i32::from(c));
    }
    for &c in &fg.ar_coeffs_cb {
        o.push(i32::from(c));
    }
    for &c in &fg.ar_coeffs_cr {
        o.push(i32::from(c));
    }
    o.push(i32::from(fg.ar_coeff_shift));
    o.push(i32::from(fg.grain_scale_shift));
    o.push(i32::from(fg.cb_mult));
    o.push(i32::from(fg.cb_luma_mult));
    o.push(i32::from(fg.cb_offset));
    o.push(i32::from(fg.cr_mult));
    o.push(i32::from(fg.cr_luma_mult));
    o.push(i32::from(fg.cr_offset));
    o.push(i32::from(fg.overlap_flag));
    o.push(i32::from(fg.clip_to_restricted_range));
    assert_eq!(o.len(), 159);
    o
}

#[test]
fn noise_table_matches_c() {
    // EB_CR_STUDIO_RANGE = 0, EB_CR_FULL_RANGE = 1 (EbSvtAv1.h).
    let mut cells = 0usize;
    for (w, h) in [(128u32, 128u32), (1280, 720), (1920, 1080), (3840, 2160), (2560, 1440)] {
        for strength in [1u32, 8, 25, 50, 120, 200] {
            for chroma in [-1i32, 0, 12] {
                for cfl in [0i32, 1] {
                    for size in [-1i32, 0, 5, 13] {
                        for full_range in [false, true] {
                            let c = cref::generate_noise_table(
                                w, h, strength, chroma, cfl, size, true,
                                full_range, false,
                            )
                            .expect("C table");
                            let r = noise_gen::generate_noise_table(
                                w,
                                h,
                                strength,
                                chroma,
                                cfl as i8,
                                size as i8,
                                full_range,
                            );
                            assert_eq!(
                                flatten(&r),
                                c,
                                "cell {w}x{h} str{strength} chroma{chroma} \
                                 cfl{cfl} size{size} full{full_range}"
                            );
                            cells += 1;
                        }
                    }
                }
            }
        }
    }
    println!("noise-gen parity: {cells} cells");
}

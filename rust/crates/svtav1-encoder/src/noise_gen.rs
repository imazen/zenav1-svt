//! [SVT_HDR_MODE] Photon-noise film-grain synthesis (`--noise*`) — port of
//! the fork's `noise_generation.c`: builds an AV1 `film_grain_params` table
//! from a luma/chroma strength + grain size, using AR coefficients
//! extracted from a reference grain clip. The GRAIN itself is synthesized
//! by the DECODER; the encoder only signals the table (SH
//! film_grain_params_present + FH film_grain_params, spec 5.9.30).
//!
//! Differentially tested against the exported C
//! `svt_av1_generate_noise_table` (tests/c_parity_noise_gen.rs) across the
//! strength/size/chroma/cfl/range grid.

pub use svtav1_entropy::obu::FilmGrainParams;

struct NoiseCoeffTable {
    lag: u8,
    shift: u8,
    c_y: [i16; 24],
    c_cb: [i16; 25],
    c_cr: [i16; 25],
}

/// AR coefficients per grain-size value, transcribed verbatim from the
/// fork's `noise_generation.c` `coeffs[]`.
static COEFFS: [NoiseCoeffTable; 14] = [
    NoiseCoeffTable { lag: 0, shift: 6, c_y: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], c_cb: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], c_cr: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
    NoiseCoeffTable { lag: 3, shift: 8, c_y: [7, -2, -15, -19, -9, -2, 1, -5, -15, -15, -30, -10, -16, -4, -14, -11, -16, 5, -14, -10, -18, -16, -24, 10], c_cb: [-10, -24, -35, -15, -28, -9, -6, -13, -42, -51, -95, -23, -18, -7, -48, -45, -117, -53, -95, -9, 16, 1, -76, -40, 21], c_cr: [-8, -23, -28, -24, -17, -22, 2, -5, -32, -58, -80, -45, -13, -1, -35, -45, -104, -50, -77, -12, -14, 6, -73, -46, 14] },
    NoiseCoeffTable { lag: 3, shift: 8, c_y: [5, -3, -15, -17, -12, -4, 2, -4, -17, -11, -32, -11, -14, -5, -15, -7, -18, 8, -17, -9, -13, -13, -24, 9], c_cb: [-23, -31, -44, -29, -32, 1, -9, -1, -43, -41, -96, -32, -16, 1, -30, -45, -112, -57, -98, -20, -11, -7, -72, -41, 38], c_cr: [-19, -22, -19, -23, -42, -14, -8, -22, -55, -38, -72, -27, -5, -17, -10, -41, -98, -46, -64, -9, -7, 2, -63, -39, -24] },
    NoiseCoeffTable { lag: 3, shift: 8, c_y: [0, -13, -20, -11, -14, -6, -2, -15, -5, -6, -26, -5, -6, -6, -17, -1, -18, 25, -14, -4, -17, -10, -21, 24], c_cb: [-23, -18, -37, -19, -9, -16, -5, -16, -52, -34, -110, -26, -7, -16, -16, -24, -111, -10, -76, -28, 22, 15, -80, -19, -47], c_cr: [-9, -12, -27, -3, -17, -25, 7, -6, -32, -19, -102, -23, -10, -4, -13, -17, -88, 5, -77, -19, 3, 22, -70, 2, 14] },
    NoiseCoeffTable { lag: 3, shift: 8, c_y: [-8, -8, -7, 2, -4, -10, -1, -13, 8, -4, -22, -11, 11, -7, -3, -3, -12, 42, -5, -7, -8, -1, -26, 43], c_cb: [-20, -4, -27, -17, -17, -15, -5, -7, -50, -23, -87, -38, -8, -19, -21, -9, -113, 23, -58, -39, -6, 4, -80, 34, 4], c_cr: [-15, 8, -20, -9, -15, -26, 3, -9, -34, -28, -79, -54, -1, -27, -28, -2, -100, 29, -59, -45, -7, 17, -85, 27, -19] },
    NoiseCoeffTable { lag: 3, shift: 8, c_y: [-8, -3, 1, 8, 0, -2, -8, -2, 6, -3, -30, -6, 7, -11, 3, 1, -31, 73, -5, -10, 7, 7, -35, 74], c_cb: [-2, -5, -4, 15, -11, -6, -19, -11, -25, -34, -76, -38, 13, -11, -10, -24, -78, 46, -32, -57, -6, 15, -79, 38, -9], c_cr: [-34, -13, -33, -6, -17, -11, -2, -13, -33, -27, -94, -15, -23, 11, -23, -12, -103, 58, -38, -24, -11, 9, -100, 59, 13] },
    NoiseCoeffTable { lag: 3, shift: 8, c_y: [-4, 4, 2, 3, 4, 7, -1, 5, 6, -7, -23, -16, 2, -6, 1, -6, -12, 65, 12, -12, 6, 0, -28, 75], c_cb: [-8, -3, 0, 5, -29, -2, 6, 3, -41, 2, -79, -27, -3, -3, -15, 5, -77, 74, -50, -28, -19, 20, -92, 85, -26], c_cr: [-6, -12, -22, -18, -4, -10, -26, 5, -41, -17, -80, -42, -18, 18, -19, 6, -67, 69, -17, -48, -6, 18, -75, 66, -20] },
    NoiseCoeffTable { lag: 3, shift: 8, c_y: [0, 7, 0, 0, 3, 3, 0, 4, -3, -3, -27, -12, 3, 3, -1, 3, -18, 80, 11, -10, -5, -1, -27, 81], c_cb: [12, 2, -32, 5, 0, -17, 14, -14, -23, -19, -87, -31, 2, -23, 2, -13, -56, 87, -30, -27, -10, -2, -63, 69, -28], c_cr: [-9, 15, -28, 9, -31, -13, -13, 31, -53, 4, -86, 2, -15, 7, -41, 17, -76, 89, -53, -19, -25, 35, -70, 79, -6] },
    NoiseCoeffTable { lag: 3, shift: 8, c_y: [3, 4, -4, 4, -1, 0, 2, 2, -5, -1, -30, -13, 3, 2, -2, 4, -14, 79, 22, -8, -10, 0, -31, 92], c_cb: [-15, 3, -33, 13, -13, -19, -5, 7, -34, 16, -93, -5, -13, -5, -23, -3, -66, 97, -32, -13, -31, 6, -56, 80, 2], c_cr: [-13, -11, -26, -14, -14, -4, -6, 20, -36, 2, -88, -25, -10, -9, -35, -2, -58, 84, -25, -32, -16, 15, -76, 74, 8] },
    NoiseCoeffTable { lag: 3, shift: 8, c_y: [2, 0, -4, 4, 2, -4, 5, 0, -3, -2, -31, -16, 5, -3, -4, 1, -13, 88, 23, -7, -8, 2, -29, 95], c_cb: [-6, -9, -43, 15, -30, -3, 5, -4, -27, 25, -78, -1, -30, -10, -24, 11, -80, 99, -37, 5, -23, 17, -83, 93, -12], c_cr: [-8, 1, -27, 5, -11, -25, -8, -10, -22, 13, -75, -10, -2, -8, -8, 14, -66, 99, -24, -14, -36, 16, -84, 114, 6] },
    NoiseCoeffTable { lag: 3, shift: 8, c_y: [5, -6, -2, 7, 4, -7, 4, -8, 6, -6, -28, -24, 12, -5, -1, 0, -16, 96, 29, -7, -7, 3, -31, 104], c_cb: [8, -10, -33, 11, -4, -7, -2, -11, -14, 10, -72, -22, 11, -15, -24, 43, -104, 123, -23, -22, -18, 12, -87, 117, -4], c_cr: [6, -15, -12, 21, -11, -20, 2, 10, -35, 26, -88, 0, 2, 3, -30, 42, -96, 125, -17, -17, -14, 37, -88, 110, 5] },
    NoiseCoeffTable { lag: 3, shift: 8, c_y: [3, -3, -2, 8, 4, -2, -1, -9, 7, -4, -31, -21, 7, -2, 0, 0, -19, 100, 30, -10, -2, 9, -37, 112], c_cb: [3, -18, -28, 31, -12, 5, -10, -16, 7, 3, -70, -27, 5, 1, -31, 10, -83, 118, -13, -21, -12, 18, -85, 118, -13], c_cr: [4, -14, -11, 5, -4, -22, 5, 10, -31, 29, -71, -20, 15, -4, -24, 19, -86, 113, -8, -37, 13, 23, -78, 107, -6] },
    NoiseCoeffTable { lag: 3, shift: 7, c_y: [0, 0, -1, 6, 4, -3, 0, -4, 4, -2, -17, -12, 5, -2, 3, -2, -10, 51, 20, -11, 4, 4, -21, 62], c_cb: [-4, -10, -9, 12, 3, -11, -9, -6, 3, 5, -37, -7, 9, -9, -15, 13, -54, 72, -17, -5, -7, 7, -33, 61, 7], c_cr: [-9, -4, -22, 14, -5, -2, -13, 4, -5, 20, -38, -4, 10, -3, -15, 15, -50, 62, -8, -14, -3, 13, -37, 63, 1] },
    NoiseCoeffTable { lag: 3, shift: 7, c_y: [-1, -2, 2, 7, 3, -1, -3, -3, 6, 0, -21, -9, 4, -1, 1, 1, -21, 60, 15, -5, -1, 9, -26, 69], c_cb: [-8, 6, -11, 10, -2, -2, -5, 4, -8, 16, -31, -8, 3, -6, -10, 18, -64, 71, -8, -5, 3, 17, -38, 70, 3], c_cr: [-4, -3, -16, 7, 6, -5, -3, 8, -14, 28, -41, -7, -1, 4, -16, 21, -66, 65, -10, -4, -12, 18, -46, 63, -4] },
];

/// C `get_output_noise` (noise_generation.c:130).
fn get_output_noise(setting_noise: i32, target_noise: f64, cutoff: i32) -> u8 {
    // C's parameter is int32_t: the double TRUNCATES at the call boundary
    // BEFORE the `> 0` test (0.46 -> 0 -> falls to the cutoff arm).
    let target = target_noise as i32;
    if target > 0 {
        target as u8
    } else if setting_noise > cutoff {
        1
    } else {
        0
    }
}

/// C `get_grain_size` (noise_generation.c:241): explicit `--noise-size`, or
/// a resolution ramp (<=720p-ish -> 0, ~1080p -> 2, >=2160p -> 13).
pub fn grain_size_for(width: u32, height: u32, noise_size: i8) -> u8 {
    if noise_size != -1 {
        return noise_size as u8;
    }
    let large_side = width.max(height) as f64;
    if large_side <= 1280.0 {
        return 0;
    }
    if large_side >= 3840.0 {
        return 13;
    }
    let p = (13.0f64 / 2.0).ln() / 4.0f64.ln();
    let normalized = (large_side - 1280.0) / 640.0;
    (2.0 * normalized.powf(p)) as u8
}

/// C `svt_av1_generate_noise_table` (noise_generation.c:292) minus the
/// config plumbing. `full_range` = the signaled color range (the C default
/// resolves to studio range unless `--color-range` or avif says otherwise).
#[allow(clippy::too_many_arguments)]
pub fn generate_noise_table(
    width: u32,
    height: u32,
    noise_strength: u32,
    noise_strength_chroma: i32,
    noise_chroma_from_luma: i8,
    noise_size: i8,
    full_range: bool,
) -> FilmGrainParams {
    let grain_size = grain_size_for(width, height, noise_size);
    let mut fg = FilmGrainParams::default();

    // ---- set_scaling_points_y (noise_generation.c:135) ----
    let range_min: i32 = if full_range { 0 } else { 16 };
    let range_max: i32 = if full_range { 255 } else { 235 };
    let range = range_max - range_min;
    let noise_setting = noise_strength as i32;
    let noise = f64::from(23 - i32::from(grain_size)) * f64::from(noise_setting) / 50.0;
    let range_ratio = f64::from(range) / 255.0;
    let ramp_size = 100.0 * range_ratio;

    fg.num_y_points = 6;
    fg.scaling_points_y[0] = [range_min as u8, 0];
    fg.scaling_points_y[1] = [
        (range_min + 6) as u8,
        get_output_noise(noise_setting, noise / 4.0, 1),
    ];
    // C truncates the (double) point positions on assignment to int.
    fg.scaling_points_y[2] = [
        (f64::from(range_min) + ramp_size * range_ratio) as u8,
        get_output_noise(noise_setting, noise, 0),
    ];
    fg.scaling_points_y[3] = [
        (f64::from(range_max) - ramp_size * range_ratio) as u8,
        get_output_noise(noise_setting, noise, 0),
    ];
    fg.scaling_points_y[4] = [
        (range_max - 6) as u8,
        get_output_noise(noise_setting, noise / 4.0, 1),
    ];
    fg.scaling_points_y[5] = [range_max as u8, 0];

    // ---- chroma (svt_av1_generate_noise, noise_generation.c:263) ----
    if noise_strength_chroma == 0 {
        fg.num_cb_points = 0;
        fg.num_cr_points = 0;
    } else {
        // set_scaling_points_uv (noise_generation.c:171).
        let noise_setting: i32 = if noise_strength_chroma == -1 {
            // C: `noise_args->str_luma * 0.6` on an int32 target — the
            // double product truncates.
            (f64::from(noise_strength as i32) * 0.6) as i32
        } else {
            noise_strength_chroma
        };
        let noise = f64::from(23 - i32::from(grain_size)) * f64::from(noise_setting) / 50.0;

        if noise_chroma_from_luma == 0 {
            let midpoint_l = 127u8;
            let midpoint_u = 129u8;
            let ramp = 4u8;
            fg.cr_mult = 192;
            fg.cb_mult = 192;
            fg.cr_luma_mult = 128;
            fg.cb_luma_mult = 128;
            fg.cr_offset = 256;
            fg.cb_offset = 256;
            fg.num_cb_points = 4;
            fg.num_cr_points = 4;
            let hi = get_output_noise(noise_setting, noise, 0);
            fg.scaling_points_cb[0] = [midpoint_l - ramp, hi];
            fg.scaling_points_cr[0] = [midpoint_l - ramp, hi];
            fg.scaling_points_cb[1] = [midpoint_l, 0];
            fg.scaling_points_cr[1] = [midpoint_l, 0];
            fg.scaling_points_cb[2] = [midpoint_u, 0];
            fg.scaling_points_cr[2] = [midpoint_u, 0];
            fg.scaling_points_cb[3] = [midpoint_u + ramp, hi];
            fg.scaling_points_cr[3] = [midpoint_u + ramp, hi];
        } else {
            fg.cr_mult = 128;
            fg.cb_mult = 128;
            fg.cr_luma_mult = 192;
            fg.cb_luma_mult = 192;
            fg.cr_offset = 256;
            fg.cb_offset = 256;
            fg.num_cb_points = 6;
            fg.num_cr_points = 6;
            let hi = get_output_noise(noise_setting, noise, 0);
            let mid = get_output_noise(noise_setting, noise / 4.0, 1);
            fg.scaling_points_cb[0] = [range_min as u8, 0];
            fg.scaling_points_cr[0] = [range_min as u8, 0];
            // FAITHFUL C QUIRK (noise_generation.c:224): the lower
            // mid-ramp writes `scaling_points_cr[1][1]` TWICE — the cb
            // value is never assigned and stays 0.
            fg.scaling_points_cb[1] = [(range_min + 6) as u8, 0];
            fg.scaling_points_cr[1] = [(range_min + 6) as u8, mid];
            fg.scaling_points_cb[2] = [(f64::from(range_min) + ramp_size) as u8, hi];
            fg.scaling_points_cr[2] = [(f64::from(range_min) + ramp_size) as u8, hi];
            fg.scaling_points_cb[3] = [(f64::from(range_max) - ramp_size) as u8, hi];
            fg.scaling_points_cr[3] = [(f64::from(range_max) - ramp_size) as u8, hi];
            fg.scaling_points_cb[4] = [(range_max - 6) as u8, mid];
            fg.scaling_points_cr[4] = [(range_max - 6) as u8, mid];
            fg.scaling_points_cb[5] = [range_max as u8, 0];
            fg.scaling_points_cr[5] = [range_max as u8, 0];
        }
    }

    let c = &COEFFS[usize::from(grain_size)];
    fg.apply_grain = true;
    fg.scaling_shift = 8;
    fg.ar_coeff_lag = c.lag;
    fg.ar_coeffs_y = c.c_y;
    fg.ar_coeffs_cb = c.c_cb;
    fg.ar_coeffs_cr = c.c_cr;
    fg.ar_coeff_shift = c.shift;
    fg.overlap_flag = true;
    fg.grain_scale_shift = 0;
    fg.chroma_scaling_from_luma = false;
    fg.clip_to_restricted_range = !full_range;
    fg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grain_size_ramp() {
        assert_eq!(grain_size_for(1280, 720, -1), 0);
        assert_eq!(grain_size_for(3840, 2160, -1), 13);
        assert_eq!(grain_size_for(1920, 1080, -1), 2);
        assert_eq!(grain_size_for(128, 128, 5), 5);
    }

    #[test]
    fn studio_range_defaults() {
        let fg = generate_noise_table(128, 128, 12, -1, 0, -1, false);
        assert!(fg.apply_grain);
        assert_eq!(fg.num_y_points, 6);
        assert_eq!(fg.scaling_points_y[0], [16, 0]);
        assert_eq!(fg.scaling_points_y[5], [235, 0]);
        assert!(fg.clip_to_restricted_range);
        assert_eq!(fg.num_cb_points, 4); // cfl=0 4-point midline shape
        assert_eq!(fg.cb_mult, 192);
        assert_eq!(fg.ar_coeff_lag, 0); // grain size 0 at 128x128
        assert_eq!(fg.ar_coeff_shift, 6);
    }

    #[test]
    fn chroma_zero_disables_uv() {
        let fg = generate_noise_table(128, 128, 12, 0, 0, -1, false);
        assert_eq!(fg.num_cb_points, 0);
        assert_eq!(fg.num_cr_points, 0);
    }
}

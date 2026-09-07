//! Public C film-grain controls and parameter validation.
use crate::entropy::obu::FilmGrainParams;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilmGrainConfig {
    /// C film_grain_denoise_strength, 0..=50; zero disables estimation.
    pub denoise_strength: u8,
    /// Replace source pixels with the Wiener result when model fitting succeeds.
    pub denoise_apply: bool,
    /// C adaptive_film_grain: choose 8/16/32 blocks by input resolution.
    pub adaptive: bool,
    /// C fgs_table. Takes precedence over photon noise and denoising. The
    /// encoder assigns the per-picture seed and forces apply_grain, as C does.
    pub table: Option<FilmGrainParams>,
    /// C ignore_ref: force INTER update_parameters even for equal tables.
    pub ignore_ref: bool,
}
impl Default for FilmGrainConfig {
    fn default() -> Self {
        Self {
            denoise_strength: 0,
            denoise_apply: false,
            adaptive: true,
            table: None,
            ignore_ref: false,
        }
    }
}
impl FilmGrainConfig {
    pub fn enabled(&self) -> bool {
        self.denoise_strength > 0 || self.table.is_some()
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.denoise_strength > 50 {
            return Err("film grain denoise strength must be in 0..=50");
        }
        if let Some(p) = &self.table {
            validate_parameters(p)?;
        }
        Ok(())
    }
}
pub fn validate_parameters(p: &FilmGrainParams) -> Result<(), &'static str> {
    if p.num_y_points > 14 || p.num_cb_points > 10 || p.num_cr_points > 10 {
        return Err("film grain scaling point count exceeds the AV1 limit");
    }
    for points in [
        &p.scaling_points_y[..p.num_y_points],
        &p.scaling_points_cb[..p.num_cb_points],
        &p.scaling_points_cr[..p.num_cr_points],
    ] {
        if points.windows(2).any(|w| w[0][0] >= w[1][0]) {
            return Err("film grain scaling point intensities must strictly increase");
        }
    }
    if !(8..=11).contains(&p.scaling_shift)
        || !(6..=9).contains(&p.ar_coeff_shift)
        || p.ar_coeff_lag > 3
        || p.grain_scale_shift > 3
    {
        return Err("film grain shift or AR lag is outside the AV1 range");
    }
    if p.cb_offset > 511 || p.cr_offset > 511 {
        return Err("film grain chroma offsets must fit nine bits");
    }
    if p.ar_coeffs_y
        .iter()
        .chain(&p.ar_coeffs_cb)
        .chain(&p.ar_coeffs_cr)
        .any(|&v| !(-128..=127).contains(&v))
    {
        return Err("film grain AR coefficients must fit signed eight bits");
    }
    Ok(())
}
/// C compares entire arrays, including inactive entries, and ignores the seed.
pub fn parameters_equal(a: &FilmGrainParams, b: &FilmGrainParams) -> bool {
    let mut a = a.clone();
    let mut b = b.clone();
    a.random_seed = 0;
    b.random_seed = 0;
    a == b
}

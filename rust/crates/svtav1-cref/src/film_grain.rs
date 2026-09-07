//! Safe, serialized access to the pinned C film-grain oracles.
use std::sync::Mutex;
static LOCK: Mutex<()> = Mutex::new(());
#[repr(C)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grain {
    pub apply_grain: i32,
    pub update_parameters: i32,
    pub scaling_points_y: [[i32; 2]; 14],
    pub num_y_points: i32,
    pub scaling_points_cb: [[i32; 2]; 10],
    pub num_cb_points: i32,
    pub scaling_points_cr: [[i32; 2]; 10],
    pub num_cr_points: i32,
    pub scaling_shift: i32,
    pub ar_coeff_lag: i32,
    pub ar_coeffs_y: [i32; 24],
    pub ar_coeffs_cb: [i32; 25],
    pub ar_coeffs_cr: [i32; 25],
    pub ar_coeff_shift: i32,
    pub cb_mult: i32,
    pub cb_luma_mult: i32,
    pub cb_offset: i32,
    pub cr_mult: i32,
    pub cr_luma_mult: i32,
    pub cr_offset: i32,
    pub overlap_flag: i32,
    pub clip_to_restricted_range: i32,
    pub bit_depth: i32,
    pub chroma_scaling_from_luma: i32,
    pub grain_scale_shift: i32,
    pub random_seed: u16,
    pub ignore_ref: i32,
}
unsafe extern "C" {
    fn ref_fg_fft(n: i32, inverse: i32, input: *const f32, temp: *mut f32, output: *mut f32);
    fn ref_fg_filter(n: i32, data: *mut f32, psd: f32);
    fn ref_fg_synthesis(
        p: *const Grain,
        y: *mut u16,
        u: *mut u16,
        v: *mut u16,
        w: i32,
        h: i32,
        ys: i32,
        cs: i32,
        depth: i32,
    );
    fn ref_fg_flat(
        data: *const u16,
        w: i32,
        h: i32,
        stride: i32,
        bs: i32,
        depth: i32,
        flat: *mut u8,
        plane: *mut f64,
        block: *mut f64,
        ox: i32,
        oy: i32,
    );
    fn ref_fg_wiener(
        y: *const u16,
        u: *const u16,
        v: *const u16,
        dy: *mut u16,
        du: *mut u16,
        dv: *mut u16,
        w: i32,
        h: i32,
        ys: i32,
        cs: i32,
        bs: i32,
        depth: i32,
        psd: *mut f32,
    ) -> i32;
    fn ref_fg_model(
        y: *const u16,
        u: *const u16,
        v: *const u16,
        dy: *mut u16,
        du: *mut u16,
        dv: *mut u16,
        w: i32,
        h: i32,
        ys: i32,
        cs: i32,
        depth: i32,
        strength: i32,
        adaptive: i32,
        p: *mut Grain,
    ) -> i32;
    fn ref_fg_solver(
        bins: i32,
        depth: i32,
        means: *const f64,
        stds: *const f64,
        count: i32,
        maxpoints: i32,
        a: *mut f64,
        b: *mut f64,
        x: *mut f64,
        points: *mut f64,
    ) -> i32;
}
pub fn fft(n: usize, inverse: bool, input: &[f32], temp: &mut [f32], out: &mut [f32]) {
    assert!([2, 4, 8, 16, 32].contains(&n));
    assert!(
        input.len() >= n * n * if inverse { 2 } else { 1 }
            && temp.len() >= n * n
            && out.len() >= 2 * n * n
    );
    unsafe {
        ref_fg_fft(
            n as i32,
            inverse as i32,
            input.as_ptr(),
            temp.as_mut_ptr(),
            out.as_mut_ptr(),
        );
    }
}
pub fn filter(n: usize, data: &mut [f32], psd: f32) {
    assert!([2, 4, 8, 16, 32].contains(&n));
    assert!(data.len() >= 2 * n * n);
    unsafe {
        ref_fg_filter(n as i32, data.as_mut_ptr(), psd);
    }
}
fn validate(data: [&[u16]; 3], w: usize, h: usize, strides: [usize; 3], depth: u8) {
    assert!(w > 0 && h > 0 && w <= 65535 && h <= 65535 && matches!(depth, 8 | 10));
    assert_eq!(strides[1], strides[2]);
    for c in 0..3 {
        let sub = usize::from(c > 0);
        assert!(
            strides[c] >= w.div_ceil(1 << sub)
                && strides[c] <= 65535
                && strides[c] * h <= i32::MAX as usize
                && data[c].len() >= strides[c] * h.div_ceil(1 << sub)
        );
    }
}
pub fn synthesis(
    p: &Grain,
    data: [&mut [u16]; 3],
    w: usize,
    h: usize,
    strides: [usize; 3],
    depth: u8,
) {
    validate(data.each_ref().map(|p| &p[..]), w, h, strides, depth);
    assert_eq!(p.bit_depth, i32::from(depth));
    for plane in &data {
        assert!(plane.iter().all(|&v| v < (1 << depth)));
    }
    assert!(
        (0..=14).contains(&p.num_y_points)
            && (0..=10).contains(&p.num_cb_points)
            && (0..=10).contains(&p.num_cr_points)
    );
    assert!(
        (0..=3).contains(&p.ar_coeff_lag)
            && (8..=11).contains(&p.scaling_shift)
            && (6..=9).contains(&p.ar_coeff_shift)
            && (0..=3).contains(&p.grain_scale_shift)
    );
    for points in [
        &p.scaling_points_y[..p.num_y_points as usize],
        &p.scaling_points_cb[..p.num_cb_points as usize],
        &p.scaling_points_cr[..p.num_cr_points as usize],
    ] {
        assert!(points.iter().flatten().all(|v| (0..=255).contains(v)));
        assert!(points.windows(2).all(|p| p[0][0] < p[1][0]));
    }
    assert!(
        p.ar_coeffs_y
            .iter()
            .chain(&p.ar_coeffs_cb)
            .chain(&p.ar_coeffs_cr)
            .all(|v| (-128..=127).contains(v))
    );
    assert!(
        [p.cb_mult, p.cb_luma_mult, p.cr_mult, p.cr_luma_mult]
            .into_iter()
            .all(|v| (0..=255).contains(&v))
    );
    assert!((0..=511).contains(&p.cb_offset) && (0..=511).contains(&p.cr_offset));
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let [y, u, v] = data;
    unsafe {
        ref_fg_synthesis(
            p,
            y.as_mut_ptr(),
            u.as_mut_ptr(),
            v.as_mut_ptr(),
            w as i32,
            h as i32,
            strides[0] as i32,
            strides[1] as i32,
            depth as i32,
        );
    }
}
pub fn flat(
    data: &[u16],
    w: usize,
    h: usize,
    stride: usize,
    bs: usize,
    depth: u8,
    ox: i32,
    oy: i32,
) -> (Vec<u8>, Vec<f64>, Vec<f64>) {
    assert!(
        w > 0 && h > 0 && stride >= w && data.len() >= stride * h && [4, 8, 16, 32].contains(&bs)
    );
    assert!(
        w <= 65535
            && h <= 65535
            && stride <= 65535
            && stride * h <= i32::MAX as usize
            && matches!(depth, 8 | 10)
    );
    assert!(ox <= i32::MAX - bs as i32 && oy <= i32::MAX - bs as i32);
    let mut flat = vec![0; w.div_ceil(bs) * h.div_ceil(bs)];
    let mut plane = vec![0.0; bs * bs];
    let mut block = plane.clone();
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        ref_fg_flat(
            data.as_ptr(),
            w as i32,
            h as i32,
            stride as i32,
            bs as i32,
            depth as i32,
            flat.as_mut_ptr(),
            plane.as_mut_ptr(),
            block.as_mut_ptr(),
            ox,
            oy,
        );
    }
    (flat, plane, block)
}
pub fn wiener(
    data: [&[u16]; 3],
    w: usize,
    h: usize,
    strides: [usize; 3],
    depth: u8,
    bs: usize,
    mut psd: [f32; 3],
) -> [Vec<u16>; 3] {
    validate(data, w, h, strides, depth);
    assert!([8, 16, 32].contains(&bs) && w >= 2 && h >= 2);
    let mut out: [Vec<u16>; 3] =
        core::array::from_fn(|c| vec![0; strides[c] * h.div_ceil(if c == 0 { 1 } else { 2 })]);
    let [y, u, v] = &mut out;
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    assert_ne!(
        unsafe {
            ref_fg_wiener(
                data[0].as_ptr(),
                data[1].as_ptr(),
                data[2].as_ptr(),
                y.as_mut_ptr(),
                u.as_mut_ptr(),
                v.as_mut_ptr(),
                w as i32,
                h as i32,
                strides[0] as i32,
                strides[1] as i32,
                bs as i32,
                depth as i32,
                psd.as_mut_ptr(),
            )
        },
        0
    );
    out
}
pub fn model(
    data: [&[u16]; 3],
    w: usize,
    h: usize,
    strides: [usize; 3],
    depth: u8,
    strength: u8,
    adaptive: bool,
    seed: u16,
) -> ([Vec<u16>; 3], Grain) {
    validate(data, w, h, strides, depth);
    assert!(w % 2 == 0 && h % 2 == 0);
    let mut out: [Vec<u16>; 3] =
        core::array::from_fn(|c| vec![0; strides[c] * h.div_ceil(if c == 0 { 1 } else { 2 })]);
    let [y, u, v] = &mut out;
    let mut grain = Grain {
        random_seed: seed,
        ..Default::default()
    };
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    assert_ne!(
        unsafe {
            ref_fg_model(
                data[0].as_ptr(),
                data[1].as_ptr(),
                data[2].as_ptr(),
                y.as_mut_ptr(),
                u.as_mut_ptr(),
                v.as_mut_ptr(),
                w as i32,
                h as i32,
                strides[0] as i32,
                strides[1] as i32,
                depth as i32,
                strength as i32,
                adaptive as i32,
                &mut grain,
            )
        },
        0
    );
    (out, grain)
}
pub fn solver(
    bins: usize,
    depth: u8,
    means: &[f64],
    stds: &[f64],
    maxpoints: i32,
) -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<[f64; 2]>) {
    assert!(
        (2..=1024).contains(&bins)
            && means.len() == stds.len()
            && !means.is_empty()
            && means.len() <= i32::MAX as usize
    );
    assert!(matches!(depth, 8 | 10));
    assert!(
        means
            .iter()
            .all(|v| v.is_finite() && *v >= 0.0 && *v <= ((1u32 << depth) - 1) as f64)
    );
    assert!(stds.iter().all(|v| v.is_finite()));
    let mut a = vec![0.0; bins * bins];
    let mut b = vec![0.0; bins];
    let mut x = b.clone();
    let mut points = vec![[0.0; 2]; bins];
    let n = unsafe {
        ref_fg_solver(
            bins as i32,
            depth as i32,
            means.as_ptr(),
            stds.as_ptr(),
            means.len() as i32,
            maxpoints,
            a.as_mut_ptr(),
            b.as_mut_ptr(),
            x.as_mut_ptr(),
            points.as_mut_ptr().cast(),
        )
    };
    assert!(n >= 0);
    points.truncate(n as usize);
    (a, b, x, points)
}

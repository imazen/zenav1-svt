//! Tier-1 differentials for `svtav1_encoder::port_preanalysis`
//! (`docs/WORKING-ON-THIS.md` §4 tier 1: the real exported C symbol, driven
//! through `svtav1-cref`).
//!
//! Covers `svt_aom_downsample_2d_c`, `calculate_histogram`,
//! `svt_aom_generate_padding`, `pad_input_picture` and
//! `svt_aom_is_input_luma_dominant`.

use svtav1_cref::preanalysis as cref;
use svtav1_encoder::port_preanalysis as port;

/// Deterministic pseudo-random bytes — a plain LCG so both sides see the exact
/// same picture and a failure is reproducible from the seed alone.
fn fill(seed: u64, n: usize) -> Vec<u8> {
    let mut s = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    (0..n)
        .map(|_| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (s >> 33) as u8
        })
        .collect()
}

#[test]
fn downsample_2d_matches_c() {
    let mut cells = 0usize;
    // decim_step 2 is the 1/4 plane; 4 is the direct-4x sixteenth route.
    for &decim_step in &[2u32, 4] {
        for &(w, h) in &[(16u32, 16u32), (64, 64), (32, 16), (96, 88), (12, 20)] {
            for &stride_pad in &[0u32, 7] {
                let stride = w + stride_pad;
                let input = fill(
                    u64::from(w) * 7717
                        + u64::from(h) * 131
                        + u64::from(decim_step) * 3
                        + u64::from(stride_pad),
                    (stride * h) as usize,
                );
                let out_w = (w / decim_step) as usize + 4;
                let out_h = (h / decim_step) as usize + 4;
                let out_stride = out_w as u32;

                let mut c_out = vec![0xABu8; out_w * out_h];
                cref::downsample_2d(&input, stride, w, h, &mut c_out, out_stride, decim_step);

                let mut r_out = vec![0xABu8; out_w * out_h];
                port::downsample_2d(
                    &input,
                    stride as usize,
                    w as usize,
                    h as usize,
                    &mut r_out,
                    out_stride as usize,
                    decim_step as usize,
                );

                assert_eq!(
                    r_out, c_out,
                    "downsample_2d mismatch: {w}x{h} stride {stride} step {decim_step}"
                );
                cells += 1;
            }
        }
    }
    assert!(cells >= 20, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn calculate_histogram_matches_c() {
    let mut cells = 0usize;
    // decim_step 1 is the scene-change-detection route; 4 is the default.
    for &decim_step in &[1u8, 4] {
        for &(w, h) in &[(16u32, 16u32), (40u32, 24u32), (7u32, 5u32)] {
            let stride = w + 3;
            let input = fill(
                u64::from(w) * 991 + u64::from(h) * 17 + u64::from(decim_step),
                (stride * h) as usize,
            );

            // C's caller seeds every bin to 1 and `sum` accumulates, so the
            // pre-state must be identical on both sides.
            let mut c_hist = [1u32; 256];
            let mut c_sum = 5u64;
            cref::calculate_histogram(&input, w, h, stride, decim_step, &mut c_hist, &mut c_sum);

            let mut r_hist = [1u32; 256];
            let mut r_sum = 5u64;
            port::calculate_histogram(
                &input,
                w as usize,
                h as usize,
                stride as usize,
                usize::from(decim_step),
                &mut r_hist,
                &mut r_sum,
            );

            assert_eq!(
                r_hist, c_hist,
                "histogram mismatch {w}x{h} step {decim_step}"
            );
            assert_eq!(
                r_sum, c_sum,
                "histogram sum mismatch {w}x{h} step {decim_step}"
            );
            cells += 1;
        }
    }
    assert_eq!(cells, 6);
}

#[test]
fn generate_padding_matches_c() {
    let mut cells = 0usize;
    // border 68 is the real luma border; 34 is the chroma one (68 >> 1);
    // the small ones keep the test cheap while exercising odd geometry.
    for &border in &[2u32, 8, 34, 68] {
        for &(w, h) in &[(16u32, 16u32), (64, 64), (96, 88)] {
            let stride = w + 2 * border + 5; // deliberate slack past the border
            let alloc = (stride * (h + 2 * border)) as usize;
            let origin = (border * stride + border) as usize;

            // Fill the WHOLE allocation with noise, not zeros: the vertical
            // copy length is `src_stride`, so it drags along whatever trails
            // the right padding. Zeroed slack would hide a wrong length.
            let base = fill(
                u64::from(w) * 313 + u64::from(h) * 29 + u64::from(border),
                alloc,
            );

            let mut c_buf = base.clone();
            cref::generate_padding(&mut c_buf, origin as u32, stride, w, h, border, border);

            let mut r_buf = base.clone();
            port::generate_padding(
                &mut r_buf,
                origin,
                stride as usize,
                w as usize,
                h as usize,
                border as usize,
                border as usize,
            );

            assert_eq!(
                r_buf, c_buf,
                "generate_padding mismatch {w}x{h} border {border}"
            );
            cells += 1;
        }
    }
    assert_eq!(cells, 12);
}

#[test]
fn pad_input_picture_matches_c() {
    let mut cells = 0usize;
    // 426x240 -> 432x240 is the case the C comment names.
    for &(w, h, pr, pb) in &[
        (426u32, 240u32, 6u32, 0u32),
        (16, 16, 0, 0),
        (13, 11, 3, 5),
        (64, 60, 0, 4),
    ] {
        let stride = w + pr + 9;
        let alloc = (stride * (h + pb + 2)) as usize;
        let base = fill(u64::from(w) * 77 + u64::from(h) * 13 + u64::from(pr), alloc);

        let mut c_buf = base.clone();
        cref::pad_input_picture(&mut c_buf, stride, w, h, pr, pb);

        let mut r_buf = base.clone();
        port::pad_input_picture(
            &mut r_buf,
            stride as usize,
            w as usize,
            h as usize,
            pr as usize,
            pb as usize,
        );

        assert_eq!(
            r_buf, c_buf,
            "pad_input_picture mismatch {w}x{h} pad {pr}/{pb}"
        );
        cells += 1;
    }
    assert_eq!(cells, 4);
}

#[test]
fn is_input_luma_dominant_matches_c() {
    let mut cells = 0usize;
    let mut saw_true = 0usize;
    let mut saw_false = 0usize;

    // Three regimes so the gate cannot be vacuous: exactly neutral chroma
    // (true), a small dither around 128 (straddles the core/tail/neutral
    // thresholds), and saturated chroma (false).
    for &(w, h) in &[(64u32, 64u32), (96, 88), (426, 240), (2, 2)] {
        let uv_w = (w >> 1) as usize;
        let uv_h = (h >> 1) as usize;
        let u_stride = uv_w + 4;
        let v_stride = uv_w + 7;
        for regime in 0..5u32 {
            let mut u = vec![0u8; u_stride * uv_h.max(1) + u_stride];
            let mut v = vec![0u8; v_stride * uv_h.max(1) + v_stride];
            for y in 0..uv_h {
                for x in 0..uv_w {
                    let (uu, vv) = match regime {
                        0 => (128u8, 128u8),
                        1 => (
                            (128 + ((x + y) % 3) as i32 - 1) as u8,
                            (128 + ((x * 2 + y) % 3) as i32 - 1) as u8,
                        ),
                        2 => (
                            (128 + ((x + y) % 17) as i32 - 8) as u8,
                            (128 + ((x + 3 * y) % 17) as i32 - 8) as u8,
                        ),
                        3 => (200, 60),
                        _ => (
                            (128 + ((x * 7 + y * 5) % 41) as i32 - 20) as u8,
                            (128 + ((x * 3 + y * 11) % 41) as i32 - 20) as u8,
                        ),
                    };
                    u[y * u_stride + x] = uu;
                    v[y * v_stride + x] = vv;
                }
            }

            // EB_YUV420 == 1
            let c = cref::is_input_luma_dominant(1, w, h, &u, u_stride as u32, &v, v_stride as u32);
            let r =
                port::is_input_luma_dominant(1, w as usize, h as usize, &u, u_stride, &v, v_stride);
            assert_eq!(r, c, "luma-dominant mismatch {w}x{h} regime {regime}");
            if c {
                saw_true += 1;
            } else {
                saw_false += 1;
            }
            cells += 1;
        }

        // EB_YUV400 == 0 is rejected outright on both sides.
        let u = vec![128u8; u_stride * uv_h.max(1)];
        let v = vec![128u8; v_stride * uv_h.max(1)];
        let c = cref::is_input_luma_dominant(0, w, h, &u, u_stride as u32, &v, v_stride as u32);
        let r = port::is_input_luma_dominant(0, w as usize, h as usize, &u, u_stride, &v, v_stride);
        assert_eq!(r, c);
        assert!(!c, "EB_YUV400 must be rejected");
        cells += 1;
    }

    assert_eq!(cells, 24);
    // Anti-vacuity: a predicate that always returned the same value would
    // pass a one-sided gate. Both outcomes must actually occur.
    assert!(saw_true > 0, "no cell was luma-dominant");
    assert!(saw_false > 0, "no cell was rejected");
}

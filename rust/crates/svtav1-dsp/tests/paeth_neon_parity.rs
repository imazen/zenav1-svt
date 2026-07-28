//! NEON Paeth prediction must be bit-identical to the scalar core.
//!
//! Paeth is a three-way argmin over distances that are frequently EQUAL, and
//! the tie-breaking order (`top`, then `left`, then `top_left`) is what makes
//! the predictor deterministic. A vectorized select that gets the tie order
//! wrong still produces plausible pixels — and would silently change every
//! intra block, breaking this crate's byte-identical-OBU bar.
//!
//! So the first test sweeps the ENTIRE (top, left, top_left) domain — 2^24
//! triples — rather than sampling, because ties are exactly where sampling
//! misses.

fn scalar_paeth(top: u8, left: u8, tl: u8) -> u8 {
    let (top, lft, tl) = (top as i32, left as i32, tl as i32);
    let base = top + lft - tl;
    let p_top = (base - top).abs();
    let p_left = (base - lft).abs();
    let p_tl = (base - tl).abs();
    if p_top <= p_left && p_top <= p_tl {
        top as u8
    } else if p_left <= p_tl {
        lft as u8
    } else {
        tl as u8
    }
}

#[test]
fn paeth_matches_scalar_over_the_entire_input_domain() {
    // 256^3 triples. Driven through a 1x1 block so every triple goes through
    // the dispatch, exercising the tail path exhaustively.
    for tl in 0..=255u8 {
        for l in 0..=255u8 {
            let above: Vec<u8> = (0..=255u8).collect();
            let left = [l];
            let mut dst = vec![0u8; 256];
            svtav1_dsp::intra_pred::predict_paeth(&mut dst, 256, &above, &left, tl, 256, 1);
            for t in 0..=255usize {
                let want = scalar_paeth(t as u8, l, tl);
                assert_eq!(
                    dst[t], want,
                    "paeth mismatch: top={t} left={l} top_left={tl}"
                );
            }
        }
    }
}

#[test]
fn paeth_matches_scalar_for_every_block_shape() {
    // Widths that are and are not multiples of 8, so both the vector body and
    // the scalar tail run at every offset.
    let mut s = 0x9e37_79b9u32;
    let mut next = move || {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (s >> 24) as u8
    };
    let above: Vec<u8> = (0..64).map(|_| next()).collect();
    let left: Vec<u8> = (0..64).map(|_| next()).collect();

    for &w in &[1usize, 2, 4, 7, 8, 9, 15, 16, 17, 32, 64] {
        for &h in &[1usize, 2, 4, 8, 16, 32, 64] {
            for &tl in &[0u8, 1, 127, 128, 254, 255] {
                let stride = w + 3;
                let mut got = vec![0u8; stride * h];
                svtav1_dsp::intra_pred::predict_paeth(&mut got, stride, &above, &left, tl, w, h);
                for row in 0..h {
                    for col in 0..w {
                        let want = scalar_paeth(above[col], left[row], tl);
                        assert_eq!(
                            got[row * stride + col], want,
                            "mismatch at {w}x{h} tl={tl} row={row} col={col}"
                        );
                    }
                }
            }
        }
    }
}

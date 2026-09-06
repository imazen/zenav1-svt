//! Direct C coverage for 8-bit DC edge selection, rounding, and row storage.
use svtav1_dsp::intra_pred::predict_dc;
#[test]
fn dc_all_shapes_edges_and_strides_match_c() {
    let shapes = [
        (4, 4),
        (8, 8),
        (16, 16),
        (32, 32),
        (64, 64),
        (4, 8),
        (4, 16),
        (8, 4),
        (8, 16),
        (8, 32),
        (16, 4),
        (16, 8),
        (16, 32),
        (16, 64),
        (32, 8),
        (32, 16),
        (32, 64),
        (64, 16),
        (64, 32),
    ];
    let mut state = 0x517cc1b7u32;
    let mut count = 0;
    for (w, h) in shapes {
        for stride in [0, w - 1, w, w + 7] {
            for edges in 0..4 {
                for pattern in 0..32 {
                    let mut above = [0u8; 67];
                    let mut left = [0u8; 67];
                    for v in above.iter_mut().chain(left.iter_mut()) {
                        state ^= state << 13;
                        state ^= state >> 17;
                        state ^= state << 5;
                        *v = match pattern {
                            0 => 0,
                            1 => 255,
                            2 => 128,
                            _ => state as u8,
                        };
                    }
                    let mut got = vec![73u8; h * stride + w + 11];
                    let mut expected = got.clone();
                    let (a, l) = (edges & 1 != 0, edges & 2 != 0);
                    predict_dc(&mut got[3..], stride, &above[3..], &left[3..], w, h, a, l);
                    svtav1_cref::dc_intra_pred(
                        &mut expected[3..],
                        stride,
                        &above[3..],
                        &left[3..],
                        w,
                        h,
                        a,
                        l,
                    );
                    assert_eq!(
                        got, expected,
                        "{w}x{h} stride={stride} edges={edges} pattern={pattern}"
                    );
                    count += 1;
                }
            }
        }
    }
    assert_eq!(count, 9728);
}

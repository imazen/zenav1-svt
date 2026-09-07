//! Frozen ARM reference kernels from zenav1-svt 74d92430 (before pairwise widening).
//! Kept only in the comparison benchmark; production calls use the crate.
use archmage::prelude::*;
pub fn sse(
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    incant!(
        sse_impl(src, src_stride, ref_, ref_stride, width, height),
        [v3, neon, scalar]
    )
}

#[arcane]
fn sse_impl_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    // Small transform blocks never enter the 16-byte body. Keep their
    // vector path separate so larger blocks retain the existing row loop.
    if width == 4 || width == 8 {
        let mut total = 0u64;
        for row in 0..height {
            let s_off = row * src_stride;
            let r_off = row * ref_stride;
            let (a, b) = if width == 8 {
                let a: &[u8; 8] = src[s_off..s_off + 8].try_into().unwrap();
                let b: &[u8; 8] = ref_[r_off..r_off + 8].try_into().unwrap();
                (vld1_u8(a), vld1_u8(b))
            } else {
                let a: &[u8; 4] = src[s_off..s_off + 4].try_into().unwrap();
                let b: &[u8; 4] = ref_[r_off..r_off + 4].try_into().unwrap();
                (
                    vcreate_u8(u64::from(u32::from_le_bytes(*a))),
                    vcreate_u8(u64::from(u32::from_le_bytes(*b))),
                )
            };
            let d = vabd_u8(a, b);
            total += u64::from(vaddlvq_u16(vmull_u8(d, d)));
        }
        return total;
    }

    // |a-b| via vabdq_u8 (exact for u8), squared with vmull_u8 into u16 and
    // drained to u32 each chunk — squares reach 65025 so they must not
    // accumulate in u16. The u32 accumulator is drained to u64 per ROW, so an
    // arbitrarily tall block cannot overflow it.
    let mut total: u64 = 0;
    let mut tail: u64 = 0;

    for row in 0..height {
        let s_off = row * src_stride;
        let r_off = row * ref_stride;
        let mut col = 0;
        let mut acc = vdupq_n_u32(0);

        while col + 16 <= width {
            let a: &[u8; 16] = src[s_off + col..s_off + col + 16].try_into().unwrap();
            let b: &[u8; 16] = ref_[r_off + col..r_off + col + 16].try_into().unwrap();
            let d = vabdq_u8(vld1q_u8(a), vld1q_u8(b));
            let lo = vget_low_u8(d);
            let hi = vget_high_u8(d);
            acc = vpadalq_u16(acc, vmull_u8(lo, lo));
            acc = vpadalq_u16(acc, vmull_u8(hi, hi));
            col += 16;
        }
        total += vaddvq_u32(acc) as u64;

        while col < width {
            let diff = src[s_off + col] as i32 - ref_[r_off + col] as i32;
            tail += (diff * diff) as u64;
            col += 1;
        }
    }
    total + tail
}

fn sse_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    let mut sse: u64 = 0;
    for row in 0..height {
        let s_off = row * src_stride;
        let r_off = row * ref_stride;
        for col in 0..width {
            let diff = src[s_off + col] as i32 - ref_[r_off + col] as i32;
            sse += (diff * diff) as u64;
        }
    }
    sse
}

#[arcane]
pub fn block_sad_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    rf: &[u8],
    ref_stride: usize,
    w: usize,
    h: usize,
) -> u32 {
    let mut acc = vdupq_n_u32(0);
    let mut tail = 0u32;
    for y in 0..h {
        let so = y * src_stride;
        let ro = y * ref_stride;
        let mut c = 0usize;
        let mut racc = vdupq_n_u16(0);
        while c + 16 <= w {
            let a: &[u8; 16] = src[so + c..so + c + 16].try_into().unwrap();
            let b: &[u8; 16] = rf[ro + c..ro + c + 16].try_into().unwrap();
            racc = vpadalq_u8(racc, vabdq_u8(vld1q_u8(a), vld1q_u8(b)));
            c += 16;
        }
        if c + 8 <= w {
            let a: &[u8; 8] = src[so + c..so + c + 8].try_into().unwrap();
            let b: &[u8; 8] = rf[ro + c..ro + c + 8].try_into().unwrap();
            racc = vaddq_u16(racc, vmovl_u8(vabd_u8(vld1_u8(a), vld1_u8(b))));
            c += 8;
        }
        acc = vpadalq_u16(acc, racc);
        while c < w {
            tail += u32::from(src[so + c].abs_diff(rf[ro + c]));
            c += 1;
        }
    }
    vaddvq_u32(acc) + tail
}

#[arcane]
pub fn block_sum_sse_neon(
    _token: NeonToken,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    w: usize,
    h: usize,
) -> (i32, u32) {
    let mut acc_a = vdupq_n_u32(0);
    let mut acc_b = vdupq_n_u32(0);
    let mut acc_sse = vdupq_n_u32(0);
    let mut tail_sum: i32 = 0;
    let mut tail_sse: u32 = 0;
    for y in 0..h {
        let ao = y * a_stride;
        let bo = y * b_stride;
        let mut c = 0usize;
        let mut ra = vdupq_n_u16(0);
        let mut rb = vdupq_n_u16(0);
        while c + 16 <= w {
            let av: &[u8; 16] = a[ao + c..ao + c + 16].try_into().unwrap();
            let bv: &[u8; 16] = b[bo + c..bo + c + 16].try_into().unwrap();
            let va = vld1q_u8(av);
            let vb = vld1q_u8(bv);
            ra = vpadalq_u8(ra, va);
            rb = vpadalq_u8(rb, vb);
            let d = vabdq_u8(va, vb);
            // |d| <= 255 so d*d <= 65025, inside u16; the widening pairwise
            // accumulate then drains into u32.
            acc_sse = vpadalq_u16(acc_sse, vmull_u8(vget_low_u8(d), vget_low_u8(d)));
            acc_sse = vpadalq_u16(acc_sse, vmull_high_u8(d, d));
            c += 16;
        }
        if c + 8 <= w {
            let av: &[u8; 8] = a[ao + c..ao + c + 8].try_into().unwrap();
            let bv: &[u8; 8] = b[bo + c..bo + c + 8].try_into().unwrap();
            let va = vld1_u8(av);
            let vb = vld1_u8(bv);
            ra = vaddq_u16(ra, vmovl_u8(va));
            rb = vaddq_u16(rb, vmovl_u8(vb));
            let d = vabd_u8(va, vb);
            acc_sse = vpadalq_u16(acc_sse, vmull_u8(d, d));
            c += 8;
        }
        acc_a = vpadalq_u16(acc_a, ra);
        acc_b = vpadalq_u16(acc_b, rb);
        while c < w {
            let d = i32::from(a[ao + c]) - i32::from(b[bo + c]);
            tail_sum += d;
            tail_sse += (d * d) as u32;
            c += 1;
        }
    }
    let sum = (vaddvq_u32(acc_a) as i32) - (vaddvq_u32(acc_b) as i32) + tail_sum;
    (sum, vaddvq_u32(acc_sse) + tail_sse)
}

// Experiment: combine adjacent narrow rows in registers, never in a staging buffer.
pub fn sse_rowpack(src: &[u8], ss: usize, rf: &[u8], rs: usize, w: usize, h: usize) -> u64 {
    rowpack_neon(NeonToken::summon().unwrap(), src, ss, rf, rs, w, h)
}

#[arcane(import_intrinsics)]
fn rowpack_neon(
    token: NeonToken,
    src: &[u8],
    ss: usize,
    rf: &[u8],
    rs: usize,
    w: usize,
    h: usize,
) -> u64 {
    use magetypes::simd::generic::u8x16;
    assert!(w == 4 || w == 8);
    let mut total = 0u64;
    let mut y = 0;
    while y + 2 <= h {
        let so = y * ss;
        let ro = y * rs;
        if w == 4 {
            let a0 = u32::from_le_bytes(src[so..so + 4].try_into().unwrap());
            let a1 = u32::from_le_bytes(src[so + ss..so + ss + 4].try_into().unwrap());
            let b0 = u32::from_le_bytes(rf[ro..ro + 4].try_into().unwrap());
            let b1 = u32::from_le_bytes(rf[ro + rs..ro + rs + 4].try_into().unwrap());
            let a = vcreate_u8(u64::from(a0) | (u64::from(a1) << 32));
            let b = vcreate_u8(u64::from(b0) | (u64::from(b1) << 32));
            let d = vabd_u8(a, b);
            total += u64::from(vaddlvq_u16(vmull_u8(d, d)));
        } else {
            let a = vcombine_u8(
                vld1_u8(src[so..so + 8].try_into().unwrap()),
                vld1_u8(src[so + ss..so + ss + 8].try_into().unwrap()),
            );
            let b = vcombine_u8(
                vld1_u8(rf[ro..ro + 8].try_into().unwrap()),
                vld1_u8(rf[ro + rs..ro + rs + 8].try_into().unwrap()),
            );
            let d = u8x16::from_repr(token, a).abs_diff(u8x16::from_repr(token, b));
            let lo = d.widen_low();
            let hi = d.widen_high();
            total += u64::from(
                ((lo * lo).pairwise_widen_add() + (hi * hi).pairwise_widen_add()).reduce_add(),
            );
        }
        y += 2;
    }
    if y < h {
        for x in 0..w {
            let d = i32::from(src[y * ss + x]) - i32::from(rf[y * rs + x]);
            total += (d * d) as u64;
        }
    }
    total
}

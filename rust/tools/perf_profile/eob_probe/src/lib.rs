#![forbid(unsafe_code)]
#[inline(never)]
pub fn baseline(scan: &[u16], q: &[i32]) -> u16 {
    for i in (0..scan.len()).rev() {
        if q[scan[i] as usize] != 0 {
            return (i + 1) as u16;
        }
    }
    0
}
#[inline(never)]
pub fn candidate(scan: &[u16], q: &[i32]) -> u16 {
    let mut end = scan.len();
    // Preserve the common immediate-hit fast path before batching zero tails.
    if end != 0 && q[scan[end - 1] as usize] != 0 {
        return end as u16;
    }
    end = end.saturating_sub(1);
    while end >= 8 {
        let s: &[u16; 8] = scan[end - 8..end].try_into().unwrap();
        let mut any = 0i32;
        for j in 0..8 {
            any |= q[s[j] as usize];
        }
        if any != 0 {
            return baseline(&scan[..end], q);
        }
        end -= 8;
    }
    baseline(&scan[..end], q)
}
#[cfg(test)]
mod tests {
    #[test]
    fn all_last_positions_and_permutations() {
        for n in [0, 1, 7, 8, 9, 15, 16, 17, 64, 256, 1024] {
            let mut scan: Vec<u16> = (0..n as u16).collect();
            let mut seed = 1234567u32;
            for _ in 0..8 {
                for i in (1..n).rev() {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    scan.swap(i, seed as usize % (i + 1));
                }
                let mut q = vec![0i32; n + 3];
                assert_eq!(super::candidate(&scan, &q), 0);
                for i in 0..n {
                    q[scan[i] as usize] = if i % 2 == 0 { i32::MIN } else { i32::MAX };
                    assert_eq!(
                        super::candidate(&scan, &q),
                        (i + 1) as u16,
                        "n={n} last={i}"
                    );
                    assert_eq!(super::baseline(&scan, &q), (i + 1) as u16);
                }
            }
        }
    }
}

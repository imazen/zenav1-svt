#![forbid(unsafe_code)]
use std::hint::black_box;
use zenbench::prelude::*;
type Kernel = fn(&[u16], &[i32]) -> u16;
fn bench(suite: &mut Suite) {
    for n in [64, 256, 1024] {
        for tail in [0, 8, n / 2, n] {
            let mut scan: Vec<u16> = (0..n as u16).collect();
            let mut seed = 7654321u32;
            for i in (1..n).rev() {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                scan.swap(i, seed as usize % (i + 1));
            }
            let mut q = vec![0; n];
            if tail < n {
                q[scan[n - tail - 1] as usize] = 1;
            }
            suite.group(format!("eob-n{n}-tail{tail}"), |g| {
                for (name, k) in [
                    ("baseline", eob_probe::baseline as Kernel),
                    (
                        "candidate",
                        if std::env::args().any(|a| a == "--control") {
                            eob_probe::baseline as Kernel
                        } else {
                            eob_probe::candidate as Kernel
                        },
                    ),
                ] {
                    let scan = scan.clone();
                    let q = q.clone();
                    g.bench(name, move |b| {
                        b.iter(|| black_box(k(black_box(&scan), black_box(&q))));
                    });
                }
            });
        }
    }
}
zenbench::main!(bench);

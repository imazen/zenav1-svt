#![forbid(unsafe_code)]
use std::hint::black_box;
use zenbench::prelude::*;
type Kernel = fn(&[i16], usize, &mut [i32]);
fn bench(suite: &mut Suite) {
    for n in [16, 32] {
        for stride in [n, n + 7, n * 2] {
            let source: Vec<i16> = (0..stride * n)
                .map(|i| ((i * 7919) % 511) as i16 - 255)
                .collect();
            let baseline: Kernel = if n == 16 {
                had_compose_probe::baseline::aom_hadamard_16x16
            } else {
                had_compose_probe::baseline::aom_hadamard_32x32
            };
            let candidate: Kernel = if std::env::args().any(|a| a == "--control") {
                baseline
            } else if n == 16 {
                had_compose_probe::candidate16
            } else {
                had_compose_probe::candidate32
            };
            suite.group(format!("had{n}-stride{stride}"), |g| {
                for (name, kernel) in [("baseline", baseline), ("candidate", candidate)] {
                    let src = source.clone();
                    g.bench(name, move |b| {
                        let src = src.clone();
                        let mut output = vec![0; n * n];
                        b.iter(move || {
                            kernel(black_box(&src), stride, black_box(&mut output));
                            black_box(&output);
                        });
                    });
                }
            });
        }
    }
}
zenbench::main!(bench);

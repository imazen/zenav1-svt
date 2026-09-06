#![forbid(unsafe_code)]
use archmage::prelude::*;
use std::hint::black_box;
use zenbench::prelude::*;
type Kernel = fn(X64V3Token, &[i32]) -> i64;
fn bench(suite: &mut Suite) {
    let t = X64V3Token::summon().unwrap();
    for n in [16, 32, 64, 256, 1024] {
        for (kind, base, candidate) in [
            (
                "wide",
                satd_probe::wide_baseline as Kernel,
                satd_probe::wide as Kernel,
            ),
            (
                "narrow",
                satd_probe::narrow_baseline as Kernel,
                satd_probe::narrow as Kernel,
            ),
        ] {
            let input: Vec<i32> = (0..n)
                .map(|i| ((i * 7919) % 65281) as i32 - 32640)
                .collect();
            suite.group(format!("{kind}-n{n}"), |g| {
                for (name, k) in [
                    ("baseline", base),
                    (
                        "candidate",
                        if std::env::args().any(|a| a == "--control") {
                            base
                        } else {
                            candidate
                        },
                    ),
                ] {
                    let input = input.clone();
                    g.bench(name, move |b| {
                        b.iter(|| black_box(k(t, black_box(&input))));
                    });
                }
            });
        }
    }
}
zenbench::main!(bench);

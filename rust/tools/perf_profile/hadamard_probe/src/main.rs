#![forbid(unsafe_code)]
use archmage::prelude::*;
use std::hint::black_box;
use zenbench::prelude::*;
type Kernel = fn(X64V3Token, &[i16], usize, &mut [i32]);
fn baseline(_: X64V3Token, input: &[i16], stride: usize, output: &mut [i32]) {
    had_probe::baseline::run(input, stride, output);
}
fn bench(suite: &mut Suite) {
    let token = X64V3Token::summon().expect("AVX2 benchmark");
    for stride in [8, 16, 32] {
        let source: [i16; 256] = core::array::from_fn(|i| ((i * 7919) % 511) as i16 - 255);
        suite.group(format!("hadamard8-stride{stride}"), |g| {
            for (name, kernel) in [
                ("baseline", baseline as Kernel),
                (
                    "candidate",
                    if std::env::args().any(|a| a == "--control") {
                        baseline as Kernel
                    } else {
                        had_probe::candidate as Kernel
                    },
                ),
            ] {
                g.bench(name, move |b| {
                    let mut output = [0i32; 64];
                    b.iter(move || {
                        kernel(token, black_box(&source), stride, black_box(&mut output));
                        black_box(&output);
                    });
                });
            }
        });
    }
}
zenbench::main!(bench);

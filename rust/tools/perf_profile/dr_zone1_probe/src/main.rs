#![forbid(unsafe_code)]
use std::hint::black_box;
use zenbench::prelude::*;
type Kernel = fn(&mut [u8], usize, usize, usize, &[u8], usize, i32);
fn bench(suite: &mut Suite) {
    for (w, h) in [(16, 16), (32, 32), (64, 64)] {
        for angle in [14, 45, 67] {
            let edge: [u8; 160] = core::array::from_fn(|i| ((i * 7919) % 256) as u8);
            let dx = dr_z1_probe::DERIVATIVE[angle] as i32;
            let candidate: Kernel = if std::env::args().any(|a| a == "--control") {
                dr_z1_probe::baseline
            } else {
                dr_z1_probe::candidate
            };
            suite.group(format!("z1-{w}x{h}-a{angle}"), |g| {
                for (name, kernel) in [
                    ("baseline", dr_z1_probe::baseline as Kernel),
                    ("candidate", candidate),
                ] {
                    g.bench(name, move |b| {
                        let mut output = vec![0; w * h];
                        b.iter(move || {
                            kernel(black_box(&mut output), w, w, h, black_box(&edge), 16, dx);
                            black_box(&output);
                        });
                    });
                }
            });
        }
    }
}
zenbench::main!(bench);

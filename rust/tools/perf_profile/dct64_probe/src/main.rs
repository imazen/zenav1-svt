#![forbid(unsafe_code)]
use archmage::prelude::*;
use zenbench::prelude::*;
use std::hint::black_box;
type Kernel=fn(X64V3Token,&[i32],&mut[i32],usize);
fn bench(suite:&mut Suite) {
    let t=X64V3Token::summon().unwrap();
    for stride in [64,71] {
        let input:Vec<i32>=(0..stride*64).map(|i|((i*7919)%511) as i32-255).collect();
        suite.group(format!("dct64-stride{stride}"),|g| {
            for (name,k) in [("baseline",dct64_const_probe::baseline as Kernel),("candidate",if std::env::args().any(|a|a=="--control"){dct64_const_probe::baseline as Kernel}else{dct64_const_probe::candidate as Kernel})] {
                let input=input.clone();
                g.bench(name,move|b| {let mut out=vec![0;4096];b.iter(||{k(t,black_box(&input),black_box(&mut out),stride);black_box(&out);});});
            }
        });
    }
}
zenbench::main!(bench);

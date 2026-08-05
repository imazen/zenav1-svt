use std::hint::black_box;
fn main() {
    println!("# fp-determinism dump v2 (black_box: no compile-time folding)");
    for qp in 46u32..=63 {
        let ex = black_box(-((black_box(qp).max(40) as f64) - 35.0) / 10.0);
        let w = (1.05 - black_box(ex).exp()) * 10000.0;
        println!("pd0_qp_th\t{}\t{:#018X}\t{}", qp, w.to_bits(), w as u32);
    }
    for qp in 0u8..=63 {
        let q = black_box(qp) as f64;
        let e = black_box((q - 12.0) / 3.0);
        println!("qp_to_lambda\t{}\t{:#018X}", qp, (0.85 * black_box(2.0_f64).powf(e)).to_bits());
    }
    for v in 1u32..=64 {
        let f = black_box(f64::from(black_box(v)));
        println!("log2\t{}\t{:#018X}", v, black_box(f).log2().to_bits());
        println!("ln\t{}\t{:#018X}", v, black_box(f).ln().to_bits());
        println!("exp_neg\t{}\t{:#018X}", v, black_box(-f / 10.0).exp().to_bits());
        println!("pow1018\t{}\t{:#018X}", v, black_box(1.018f64).powf(black_box(f)).to_bits());
        println!("sqrt\t{}\t{:#018X}", v, black_box(f).sqrt().to_bits());
    }
}

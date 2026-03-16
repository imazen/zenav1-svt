//! Performance report — prints DSP function throughput.
//!
//! Run with: cargo run -p svtav1-dsp --features std --example perf_report --release

fn main() {
    println!("SVT-AV1 Rust DSP Performance Report");
    println!("====================================\n");

    println!("SAD throughput:");
    let sad_results = svtav1_dsp::bench::bench_sad_throughput();
    svtav1_dsp::bench::print_bench_results(&sad_results);

    println!("\nForward Transform throughput:");
    let txfm_results = svtav1_dsp::bench::bench_fwd_txfm_throughput();
    svtav1_dsp::bench::print_bench_results(&txfm_results);

    println!("\n--- Summary ---");
    for r in sad_results.iter().chain(txfm_results.iter()) {
        if let Some(mpix) = r.mpix_per_sec {
            println!("{}: {:.0} Mpix/s ({} ns/iter)", r.name, mpix, r.ns_per_iter);
        }
    }
}

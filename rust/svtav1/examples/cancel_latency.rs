//! Cancellation-latency harness: how long does `cancel()` actually take to
//! stop an in-flight encode?
//!
//! The cooperative-stop plumbing (`EncodePipeline::with_stop`, the guarded
//! `stop.check()` sites) has a CORRECTNESS test
//! (`pipeline::tests::try_encode_cancellation_mid_frame_is_clean_err`) proving
//! a fired token yields `Err(Cancelled)` rather than a panic or partial
//! output. It has never had a LATENCY measurement. A token that is only
//! polled between whole frames — or only inside one of a dozen phases — is
//! correct and useless: the caller waits out the phase it happened to land in.
//!
//! # What is measured
//!
//! Wall time from the instant the caller asks to stop (`Instant::now()`
//! immediately before flipping the token) to the instant
//! `try_encode_frame_420` returns — **regardless of whether the encode
//! honoured the cancel**. That is the number a caller feels. An encode that
//! runs a further 90 ms and then returns `Ok` has not cancelled in 0 ms; it
//! has cancelled in 90 ms and thrown the answer away, or not cancelled at all.
//! Scoring only the `Err(Cancelled)` cells would hide exactly the failure this
//! harness exists to find: a phase that polls no token, where a cancel landing
//! inside it is ignored until the phase ends.
//!
//! Both populations are therefore reported: `honoured` (returned
//! `Err(Cancelled)`) and `ignored` (returned `Ok` — the cancel arrived after
//! the encode's last poll). The percentile columns cover ALL cells; the
//! `honoured`/`ignored` split says which kind of wait it was.
//!
//! # Method
//!
//! Per size: one untimed warmup encode, then `--baseline-reps` timed
//! uncancelled encodes whose MINIMUM is the reference duration `T`. Then, for
//! each cancel point `f` in an evenly spaced sweep over `(0, 1)` and each
//! repetition, a fresh pipeline is built (untimed), a canceller thread is
//! spawned that sleeps `f * T` and then stops the token, and the encode is
//! run. A cell is scored whenever the ask preceded the return; the rare cell
//! where the encode finished BEFORE the canceller thread even asked measured
//! nothing and is dropped (`wait_ns = NA`).
//!
//! Percentiles are nearest-rank (p50 = element at `ceil(0.50 * n) - 1` of the
//! sorted sample), so a small `n` degrades honestly instead of interpolating.
//!
//! # Measurement hygiene
//!
//! Do NOT run this under `nice -n 19` on macOS: Darwin maps that to background
//! QoS and schedules the process on efficiency cores, which distorts wall
//! clock by a large factor. Build under `nice`, run without it. The harness
//! prints the nice value it observed into the TSV header so a mis-run is
//! visible in the artifact rather than silently believed.
//!
//! Usage:
//!   cancel_latency [--sizes 64,256,1024] [--preset 8] [--qp 40] [--points 12]
//!                  [--reps 3] [--baseline-reps 2] [--out FILE] [--verbose]

use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};
use svtav1_types::error::EncodeError;

/// A caller-flipped stop token. `may_stop()` is unconditionally `true` so the
/// guarded in-encode checks (`if stop.may_stop() { stop.check()? }`) actually
/// run — the default `Unstoppable` token short-circuits them away.
#[derive(Clone)]
struct Flag(Arc<AtomicBool>);

impl Flag {
    fn new() -> Self {
        Flag(Arc::new(AtomicBool::new(false)))
    }
    fn stop(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

impl enough::Stop for Flag {
    fn check(&self) -> Result<(), enough::StopReason> {
        if self.0.load(Ordering::Relaxed) {
            Err(enough::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
    fn may_stop(&self) -> bool {
        true
    }
}

/// Deterministic gradient content — the same recipe `perf_encode.rs` and
/// `identity_run.rs` use, so a cell here is comparable to a cell there.
fn gen_planes(w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            y[r * w + c] = (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8);
        }
    }
    let (cw, ch) = (w / 2, h / 2);
    let mut u = vec![0u8; cw * ch];
    let mut v = vec![0u8; cw * ch];
    for r in 0..ch {
        for c in 0..cw {
            u[r * cw + c] = 128u8.wrapping_add(((r * 7 + c) & 0x1f) as u8);
            v[r * cw + c] = 128u8.wrapping_sub(((r + c * 5) & 0x1f) as u8);
        }
    }
    (y, u, v)
}

fn build(w: usize, h: usize, preset: u8, qp: u8) -> EncodePipeline {
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp,
        ..RcConfig::default()
    };
    EncodePipeline::new(w as u32, h as u32, preset, rc, 0, 1)
        .with_bit_depth(8)
        .with_tile_rows_log2(0)
        .with_tile_cols_log2(0)
        .with_sb_size(None)
        .with_chroma_420(true)
}

/// Nearest-rank percentile over an already-sorted slice.
fn pct(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((p * sorted.len() as f64).ceil() as usize).max(1);
    sorted[rank.min(sorted.len()) - 1]
}

fn arg<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
}

struct Cell {
    size: usize,
    frac: f64,
    /// Ask-to-return wall time. `None` only when the encode returned BEFORE the
    /// canceller thread ever asked — that cell measured nothing and is dropped.
    latency_ns: Option<u128>,
    /// `true` when the encode returned `Err(Cancelled)`; `false` when it ran to
    /// completion and returned `Ok` despite the pending cancel.
    honoured: bool,
    total_ns: u128,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let sizes: Vec<usize> = arg(&args, "--sizes")
        .unwrap_or("64,256,1024")
        .split(',')
        .map(|s| s.trim().parse().expect("--sizes wants integers"))
        .collect();
    let preset: u8 = arg(&args, "--preset")
        .unwrap_or("8")
        .parse()
        .expect("--preset");
    let qp: u8 = arg(&args, "--qp").unwrap_or("40").parse().expect("--qp");
    let points: usize = arg(&args, "--points")
        .unwrap_or("12")
        .parse()
        .expect("--points");
    let reps: usize = arg(&args, "--reps").unwrap_or("3").parse().expect("--reps");
    let base_reps: usize = arg(&args, "--baseline-reps")
        .unwrap_or("2")
        .parse()
        .expect("--baseline-reps");
    let verbose = args.iter().any(|a| a == "--verbose");
    let out_path = arg(&args, "--out").map(|s| s.to_string());

    assert!(points >= 1 && reps >= 1 && base_reps >= 1);

    // Surface the macOS background-QoS trap in the artifact itself.
    let niceness = unsafe_free_nice();

    let mut rows: Vec<Cell> = Vec::new();
    let mut baselines: Vec<(usize, u128)> = Vec::new();

    for &n in &sizes {
        assert!(n % 2 == 0, "sizes must be even (4:2:0 chroma)");
        let (y, u, v) = gen_planes(n, n);

        // Untimed warmup, then the min of `base_reps` timed uncancelled runs.
        let _ = build(n, n, preset, qp).encode_frame_420(&y, &u, &v, n);
        let mut base = u128::MAX;
        for _ in 0..base_reps {
            let mut p = build(n, n, preset, qp);
            let t = Instant::now();
            let out = p.encode_frame_420(&y, &u, &v, n);
            base = base.min(t.elapsed().as_nanos());
            std::hint::black_box(out);
        }
        baselines.push((n, base));
        eprintln!(
            "[baseline] {n}x{n} preset={preset} qp={qp}: {:.3} ms",
            base as f64 / 1e6
        );

        for i in 1..=points {
            let frac = i as f64 / (points + 1) as f64;
            let delay = Duration::from_nanos((base as f64 * frac) as u64);
            for _ in 0..reps {
                let flag = Flag::new();
                let mut p = build(n, n, preset, qp).with_stop(flag.clone());
                // The canceller records the ask-instant itself, so the measured
                // interval starts where the CALLER asked, not where the main
                // thread noticed.
                let stamp = Arc::new(std::sync::Mutex::new(None::<Instant>));
                let stamp_w = Arc::clone(&stamp);
                let flag_w = flag.clone();
                let hdl = std::thread::spawn(move || {
                    std::thread::sleep(delay);
                    *stamp_w.lock().expect("stamp mutex poisoned") = Some(Instant::now());
                    flag_w.stop();
                });
                let t = Instant::now();
                let res = p.try_encode_frame_420(&y, &u, &v, n);
                let ret = Instant::now();
                hdl.join().expect("canceller thread panicked");
                let total_ns = ret.duration_since(t).as_nanos();
                let asked = *stamp.lock().expect("stamp mutex poisoned");
                let honoured =
                    matches!(&res, Err(e) if matches!(e.error(), EncodeError::Cancelled(_)));
                if let Err(e) = &res {
                    assert!(
                        honoured,
                        "the only error this harness may provoke is Cancelled; got {e:?}"
                    );
                }
                // Only cells where the ask genuinely preceded the return say
                // anything about cancellation latency.
                let latency_ns = asked
                    .filter(|a| *a <= ret)
                    .map(|a| ret.duration_since(a).as_nanos());
                if verbose {
                    eprintln!(
                        "  {n}x{n} f={frac:.3} total={:.3}ms wait={} {}",
                        total_ns as f64 / 1e6,
                        latency_ns
                            .map(|l| format!("{:.3}ms", l as f64 / 1e6))
                            .unwrap_or_else(|| "n/a".into()),
                        if honoured { "honoured" } else { "IGNORED(Ok)" }
                    );
                }
                rows.push(Cell {
                    size: n,
                    frac,
                    latency_ns,
                    honoured,
                    total_ns,
                });
            }
        }
    }

    // ---- report -----------------------------------------------------------
    let mut tsv = String::new();
    tsv.push_str(&format!(
        "# cancel_latency preset={preset} qp={qp} points={points} reps={reps} \
         baseline_reps={base_reps} nice={niceness}\n"
    ));
    tsv.push_str("size\tfrac\twait_ns\ttotal_ns\thonoured\n");
    for r in &rows {
        tsv.push_str(&format!(
            "{}\t{:.4}\t{}\t{}\t{}\n",
            r.size,
            r.frac,
            r.latency_ns
                .map(|l| l.to_string())
                .unwrap_or_else(|| "NA".into()),
            r.total_ns,
            u8::from(r.honoured)
        ));
    }

    println!(
        "\nsize\tbaseline_ms\tn\thonoured\tignored\tp50_ms\tp90_ms\tp99_ms\tmax_ms\tover_20ms\tworst_frac"
    );
    for &(n, base) in &baselines {
        let cells: Vec<&Cell> = rows
            .iter()
            .filter(|r| r.size == n && r.latency_ns.is_some())
            .collect();
        let mut lat: Vec<u128> = cells.iter().filter_map(|r| r.latency_ns).collect();
        let honoured = cells.iter().filter(|r| r.honoured).count();
        let ignored = cells.len() - honoured;
        lat.sort_unstable();
        let over = lat.iter().filter(|&&l| l > 20_000_000).count();
        let worst = cells.iter().max_by_key(|r| r.latency_ns.unwrap_or(0));
        println!(
            "{n}\t{:.3}\t{}\t{}\t{}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{}\t{}",
            base as f64 / 1e6,
            cells.len(),
            honoured,
            ignored,
            pct(&lat, 0.50) as f64 / 1e6,
            pct(&lat, 0.90) as f64 / 1e6,
            pct(&lat, 0.99) as f64 / 1e6,
            lat.last().copied().unwrap_or(0) as f64 / 1e6,
            over,
            worst
                .map(|r| format!("{:.3}", r.frac))
                .unwrap_or_else(|| "-".into())
        );
    }

    if let Some(p) = out_path {
        let mut f = std::fs::File::create(&p).expect("create --out");
        f.write_all(tsv.as_bytes()).expect("write --out");
        eprintln!("wrote {p}");
    }
}

/// The process's scheduling niceness, read without `unsafe` by shelling out —
/// this is a measurement-hygiene annotation, not a hot path. Returns `?` if it
/// cannot be determined.
fn unsafe_free_nice() -> String {
    std::process::Command::new("ps")
        .args(["-o", "nice=", "-p"])
        .arg(std::process::id().to_string())
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "?".into())
}

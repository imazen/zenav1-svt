# Hadamard 8x8 probe

The frozen baseline reproduces the V3 entry and column-buffer algorithm from
9adba930. The candidate uses existing generic i16x8 arithmetic and fixed-array
transposes. Its held token has a favorable entry cost relative to the baseline's
runtime dispatch; frame A/B measurements decide whether the change helps the
encoder. Both kernels are checked against the real C oracle, including padding,
10-bit residuals, full-range i16 values, and output sentinels.

Run serially through `~/work/claudehints/scripts/run-heavy --mem 16G --jobs 4`
on i265, with `TMPDIR=$HOME/tmp` and an external `CARGO_TARGET_DIR`. Use baseline
CPU flags. The benchmark is x86 V3-specific and fails if that token is unavailable.

Zenbench 0.1.9 on this Linux host detected its own `zenbench-exclusive-heartbeat`
thread as a competing benchmark. An unmodified control stalled; the same control
with `zenbench-own-threads.patch` completed with zero gate waits. The patch excludes
only threads in `/proc/self/task`, retaining resource and other-process checks.
It is a local measurement workaround, not a published Zenbench change.

Copy the source fetched by `cargo read zenbench` to a scratch directory, apply
this patch there with `patch -p1`, then pass a Cargo override to both commands:

```bash
cargo test --manifest-path rust/tools/perf_profile/hadamard_probe/Cargo.toml \
  --config 'patch.crates-io.zenbench.path="/absolute/path/to/patched-zenbench"' --lib
cargo build --release --manifest-path rust/tools/perf_profile/hadamard_probe/Cargo.toml \
  --config 'patch.crates-io.zenbench.path="/absolute/path/to/patched-zenbench"'
```

Run the resulting binary under the heavy wrapper and `taskset -c 2`, first with
`--control --format=json`, then with `--format=json`. `--control` calls the same
baseline in both slots. Save full output. The embedded `zenbench::main!` does not
implement `--help`; that argument starts measurements. Check completed group and
round counts and gate waits in the JSON, not merely the exit status.

The September 6 records distinguish the original production-linked probe from
this archived frozen-baseline probe. Both controls and both A/B runs were measured;
the archived A/B reduced kernel time by 53–55% on strides 8, 16, and 32. A staged
1/2/4-word interleave transpose was also tested but gave no further benefit.
The production all-tier test and frame measurements are recorded separately in
`rust/docs/STILL-PERF-2026-09-06.md`.

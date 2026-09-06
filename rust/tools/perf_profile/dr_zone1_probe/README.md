# Contiguous zone-one prediction probe

The frozen baseline is the non-upsampled scalar core from encoder source
d081b8e5. The candidate dispatches through existing V3 support, separates
interpolation from fill, and uses bounded u16 arithmetic. No raw intrinsics
or Archmage API additions are involved.

`cargo test --lib -- --nocapture` checks2,970 real-C cases, including padded
strides and output sentinels. `cargo run --release -- --control --format=json`
runs same-function controls; omit `--control` for A/B. Pin a performance core
and use run-heavy. The recorded runs patch zenbench to the preserved local
gate-tuned checkout via Cargo `--config`, with provenance in the earlier
Hadamard probe record. Use baseline CPU flags.

The production candidate only adds V3 to the existing width>=16,
non-upsampled guarded dispatch. The archived probe also tests smaller sizes
for arithmetic equivalence; its benchmark measures16/32/64 squares.

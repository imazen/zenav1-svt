# Mainline tune sharpness — issue #17

C `svt_av1_pick_filter_level` in `deblocking_filter.c` applies the VQ and
FILM_GRAIN sharpness increment on key frames in both mainline and HDR mode.
The Rust pipeline incorrectly restricted the adjustment to the HDR fork.
It now applies the shared derivation in both modes, with the key-frame guard
for VQ/FILM_GRAIN. IQ/MS_SSIM retain their qindex-dependent cap for all frames.
The one effective value feeds filter search, application and frame syntax.

The enabled witness is gradient 72x88, QP 40, preset 8, 8-bit 4:2:0.
Before the fix, tune 0 produced 289-byte C and Rust streams with different
contents; tune 1 matched. After the fix both tunes match C byte-for-byte.
A mutation restoring the old fork gate repeats the failure, followed by a
restored-code pass. Artifacts remain under `~/tmp/svt-tracking/tune-before/`
and `tune-after/`; the explicit directories keep both arms.

The witness is included in `tools/regression_spotcheck.sh`; it does not
compare only Rust settings against each other. The complete spotcheck passes
124/124. Workspace nextest with `zenav1-svt/avif-container` passes 2616/2616,
zero skipped. These checks do not establish every tune/preset combination.
No byte pin, quality threshold or test skip changed.

The broader clippy invocation exposed existing build-script documentation and
PD0 visibility diagnostics; their correction/validation is recorded separately.
Screen-content control wiring remains separate work under #17 and #21.

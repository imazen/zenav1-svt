# Reference-specific chroma ranking witnesses

These inputs are synthetic correctness witnesses, not RD benchmarks.
`diag64-8.yuv` is the exact I420 input retained by the 1,100-cell identity
sweep for `diag 64 64 48 0`. The native10 variant keeps the top eight bits
and adds `(sample_index * 3) % 4` as its low two bits; samples are little-endian.

The OBU files come from independently linked native C APIs, not from Rust:
pristine v4.2.0 at `9292ec8e32bce26f781f277ec8739b53426c4300` and the pinned
hybrid at `3115c0c1b23e860dfd75c94f6740e0298182dd13`, HDR mode0. Both use
AVIF/all-intra, CQP QP48, AQ0, PSNR default, LP1, 30/1 fps, unspecified CICP,
and the filename's bit depth/native preset. At preset0, both depths distinguish
the two references (native10 differs in bytes despite equal lengths).

`provenance.json` retains input/output/binary hashes and exact capture commands.
The pristine API driver was built from `capture_c_trace.c` with its fork-setter
calls omitted, against pristine headers and library, without linker interposers.
The hybrid control uses the unmodified driver without interposers. No fork
environment overrides were used. `docs/PARITY-REFERENCE-AUDIT-2026-09-08.md`
records the independent CLI/API controls and the single-branch root cause.

`pristine_and_hybrid_chroma_reference_matches_c` checks both targets, native8/10
and −1/0; compares the public wrapper too at8-bit; and independently decodes
all eight outputs against the encoder reconstruction. Do not regenerate these
goldens from Rust or silently replace one C source with the other.

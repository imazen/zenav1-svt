# zenav1-svt — handoff

Pure-Rust reimplementation of SVT-AV1 v4.2.0's **still-image / AVIF (all-intra)**
encoder. The bar is not "it encodes" — it is **byte-identical OBUs to the C
encoder at a matched config**, proven cell by cell against the real
`libSvtAv1Enc`. Everything below assumes that bar.

Last updated 2026-07-25.

---

## 1. Get it building on a fresh box

```bash
git clone --recurse-submodules https://github.com/imazen/zenav1-svt
cd zenav1-svt
# the C reference IS the oracle — it is a submodule, not a vendored copy
git submodule update --init --recursive
```

**Prerequisites**: a Rust toolchain (stable; workspace MSRV 1.85, 2024 edition),
`cmake` + `ninja` + a C compiler, and `cargo-nextest`
(`cargo install cargo-nextest`). Everything is `#![forbid(unsafe_code)]`.

**Build the C oracle** — nothing that matters can be verified without it.
Since 2026-08-27 (issue #4) cargo does it: `crates/svtav1-cref/build.rs`
configures and builds both variants on first use, SHA-stamped.

```bash
cd rust && cargo build -p zenav1-svt-cref    # or simply: cargo test
# hand-typed equivalent (same flags):
# cmake -S reference/svt-av1 -B cbuild-static -G Ninja -DCMAKE_BUILD_TYPE=Release \
#   -DCMAKE_OUTPUT_DIRECTORY="$PWD/Bin/Release/" \
#   -DBUILD_SHARED_LIBS=OFF -DBUILD_APPS=ON -DBUILD_TESTING=OFF -DSVT_AV1_LTO=OFF -DNATIVE=OFF
# cmake --build cbuild-static
```

That produces `Bin/Release/libSvtAv1Enc.a` (linked by the `svtav1-cref` crate's
FFI shims) and `Bin/Release/SvtAv1EncApp` (the CLI used to capture ground truth).

**Build a reference DECODER** — several gates require one; there is no fallback
and no "graceful skip":

```bash
git clone --depth 1 -b v3.12.1 https://aomedia.googlesource.com/aom aom-src
cmake -S aom-src -B aom-build -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DCONFIG_AV1_ENCODER=0 -DENABLE_TESTS=0 -DENABLE_EXAMPLES=1 \
  -DENABLE_DOCS=0 -DENABLE_TOOLS=0
cmake --build aom-build --target aomdec
```

Gates take `AOMDEC=/path/to/aomdec` (and optionally `DAV1D=$(command -v dav1d)`).

**Run the tests**: `cd rust && just test` — which is `cargo nextest run
--workspace` plus a doctest pass. Use nextest, not `cargo test`: it runs each
test in its own process, which is what keeps the archmage dispatch-tier state
from leaking between tests (see `rust/CLAUDE.md` → Archmage Rules).

## 2. How correctness is actually established

`rust/tools/` holds the gates. They shell out to BOTH encoders and compare
bytes. CI (`.github/workflows/rust-gates.yml`) runs SIX of them —
`bd10_matrix`, `bd10_nonflat_gate`, `bd10_hbd_src_gate`, `superres_gate`,
`recon_parity`, `decode_conformance` — plus the workspace tests. Everything
else is LOCAL-ONLY, including the flagship `identity_matrix.sh` and every
corpus-dependent gate. Run them by hand before any release.

Two traps worth knowing before you trust a gate's exit code:
`identity_matrix.sh` is a tracking SCOREBOARD — it always exits 0, so read its
printed tally, not `$?`. And gates whose corpus is missing may run a reduced
cell set; check the printed count against the documented one.

| gate | what it proves |
|---|---|
| `identity_matrix.sh` | 8-bit byte-parity vs C over content × size × qp × preset |
| `bd10_matrix.sh`, `bd10_nonflat_gate.sh`, `bd10_photo_gate.sh` | 10-bit byte-parity (source = widened 8-bit) |
| `bd10_hbd_src_gate.sh` | 10-bit byte-parity with a **real** 10-bit source (low bits set) |
| `superres_gate.sh` | superres byte-parity + decodability at the upscaled size |
| `recon_parity.sh` | encoder recon == the reference decoder's output, bit-exact |
| `decode_conformance.sh` | every stream decodes under aomdec + dav1d |

**The debugger is `tools/identity_diff.sh <w> <h> <qp> <preset> <content>`.** It
runs both encoders and reports the FIRST divergence with its stage (sequence
header field / frame header field / tile-op index + symbol). Start there for any
"the bytes differ" question — it turns a diff into a coordinate.

**Isolation trick that pays for itself**: when a feature-ON encode diverges,
dump the exact pixels the feature produces and re-encode them with the feature
OFF via `raw:<file>` content. If that matches C, the feature's inputs are fine
and the divergence is in a feature-conditional decision. This is how the
superres stale-variance root cause was found.

**Harness env vars** (the Rust side and the C driver take matched pairs):

| Rust (`tools/identity_run`) | C (`tools/capture_c_trace`) | meaning |
|---|---|---|
| `SVTAV1_BD=10` | positional bit-depth arg | encode at 10-bit |
| `SVTAV1_HBD_SRC=1` | (same .yuv) | generate a REAL 10-bit source |
| `SVTAV1_SUPERRES=<9..16>` | `SVT_SUPERRES_KF_DENOM=<9..16>` | superres denominator |
| `SVTAV1_TILE_ROWS_LOG2` | `SVT_TILE_ROWS` | tile rows (log2 on both) |
| `SVTAV1_MONO=1` | — | monochrome (no C oracle exists — see below) |
| `SVTAV1_SB=64\|128` | — | pin the superblock size |

## 3. Where the port stands

Read `rust/README.md` for the capability surface and `rust/docs/*-port-map.md`
for per-feature plans. The short version:

- **8-bit 4:2:0 still/AVIF**: byte-identical to C across the tracked matrix.
- **10-bit**: native u16 input is wired end-to-end (MD, coded levels, and the
  deblock/CDEF/LR searches). Envelope: 64-aligned dims, preset ≥ 9 or a
  full-RD-capable preset ≤ 8. Out-of-envelope configs are REJECTED, not
  silently truncated.
- **Superres**: opt-in `EncodePipeline::with_superres(denom)`, byte-identical
  to C over the gated envelope. Refused at presets ≤ 6 and at 10-bit.
- **Monochrome** is validated by decode-conformance + recon-parity, NOT by
  byte-identity: C v4.2.0 cannot encode mono, so no oracle exists. Never try to
  build one without patching C first.

`rust/CLAUDE.md` is the working agreement — read its "C v4.2.0 SHIPPING
ENVELOPE" guards before claiming any gap; each of those six facts was derived
wrongly at least once before being settled against source.

## 4. Open work, in priority order

Each item below is stopped at a KNOWN point with the evidence recorded — none
of them needs re-discovery, only execution.

1. **Tune IQ — one coefficient symbol from done.** `rust/docs/tune-iq-port-map.md`
   is the file to read. `--tune 3` is fully wired (C's seven-setting override
   block, `max_tx_size`, the scm force, the still-image QM polynomial, the
   mainline variance-boost kernel) and on `gradient 64x64 q55 p8` the encoded
   SIZE matches C exactly while the first divergence sits at tile-op 197 — the
   header, partition, skip, delta-q value and sign, y-mode and ~190 coefficient
   symbols all match. What differs is one coefficient level class (C `s=0`,
   port `s=1`). QM level selection is RULED OUT (qm levels are header fields
   and the header matches). Leading candidate: `sharpness = 7`'s effect on the
   RDOQ rshift — a path this port documents as departing from mainline at
   `>= 3` and which has never been byte-verified. Then QM application.
   When it closes, add `tools/tune_iq_gate.sh` shaped like `superres_gate.sh`
   (byte-parity at `SVT_TUNE=3` x preset x qp x content, anti-vacuity vs the
   tune-1 stream) and wire it into CI.
2. **Superres B.5 — loop restoration under superres.** The RU geometry is
   ported and tested; the ordering question that used to block it is ANSWERED
   in `rust/docs/superres-port-map.md`: C saves BOTH boundary-line sets at the
   CODED width BEFORE the upscale (`cdef_process.c:707` saves, `:715` upscales;
   `dlf_process.c:134` for the deblock set), so LR runs on the UPSCALED recon
   with CODED-width stripe boundaries, against the UNSCALED source
   (`rest_process.c:271`). Remaining work is mechanical. Presets <= 6 stay
   refused until a cell byte-matches.
3. **The one open superres cell**, `gradient_64_q32_p7_d10` — 639/640 of the
   sweep passes. Geometry, the stale-variance model and the screen-content
   source are all ruled out by measurement (see the port map). Preset 7 is the
   only allintra preset where scm-3 detection is live.
4. **Issue #9** — the rest of the defaults/recommended-settings audit:
   `max_tx_size` is now in, but fractional CRF, `chroma_sample_position`, and
   `AvifEncoder::encode_yuv420` (which returns a three-mono-stream blob, not an
   AV1 bitstream) are open.
5. **Issue #8** — documentation debt: the drifted `PORT-NOTE(unverified)` index,
   contradictory HDR verification claims, measured numbers with no committed
   artifact, and the undocumented corpora that make several headline gate
   numbers unreproducible off this box.
6. **#95 partial superblocks**, **#71 palette / IBC calibration**, **#91 SB128
   residuals**, **#93 performance** (last).

### Test-coverage gaps worth knowing before you trust a green run

- **Neither variance-boost kernel has a differential test.**
  `c_parity_var_boost.rs` covers only the qindex<->q conversions, which is
  exactly why a wrong-input-domain kernel sat there returning 0. Both C symbols
  are `static`, so testing them needs the `ref_shims.c` wrapper treatment the
  palette statics got. Do this before trusting the next tune-IQ fix.
- CI runs six of ~32 gates (see section 2). The flagship `identity_matrix.sh`
  is NOT among them, and it always exits 0 — read its tally.

## 5. Machine-local state a new box will NOT inherit

- `Bin/Release/` and `cbuild-static/` are build outputs — rebuild per §1.
- `rust/benchmarks/*.tsv` gate scoreboards are committed; the `.bak` file and
  the modified `perf_2026-07-20.*` in the working tree belong to a performance
  session and are intentionally left uncommitted.
- **A sibling repo on this box (`/root/aom-rs`, the libaom port) had 11 agent
  worktrees whose commits existed ONLY locally.** They were pushed to
  `preserve/2026-07-25-<worktree>` branches on `imazen/zenav1-aom` on 2026-07-25
  so a box handoff cannot lose them. Two of those worktrees ALSO have
  uncommitted edits that were deliberately left alone (they belong to other
  sessions): `agent-a788dbb3aaec3a1dc` (`crates/aom-encode/src/allintra_vis.rs`)
  and `wf_81e71052-dd6-5` (`crates/aom-dsp/src/restore/{frame,sgr,wiener}.rs`
  plus two new files). Decide those with their owner before wiping the box.

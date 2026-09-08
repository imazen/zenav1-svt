# zenav1-svt

Experimental pure-Rust port of SVT-AV1 v4.2.0, with explicit C-reference
selection and opt-in HDR-fork/Zen extensions. The product supports 8/10-bit
4:2:0 still encoding and all-intra animated AVIF, plus Rust monochrome/alpha
extensions. General public streaming video is unfinished and refuses calls.
There is no C dependency in the product library path.

Start with [the current handoff](CONTEXT-HANDOFF.md),
[the feature/support table](rust/docs/API-SUPPORT-AUDIT-2026-09-08.md), and
[the remaining-work tracker](https://github.com/imazen/zenav1-svt/issues/21).

## References, policy and coverage

- Legacy constructors retain **Hybrid3115**, the historical patched-C oracle.
  This is not synonymous with pristine mainline, even with HDR knobs off.
- `SvtReference::Mainline420` names pristine v4.2.0. `SvtParity(reference)`
  restricts the request to the named C envelope and rejects Zen enhancements.
  It does not certify every untested combination or erase known divergences.
- Checked native presets include **−1 through 13**. Effort currently resolves
  to native buckets; fractional adaptive search is not implemented.
- Grain modeling/denoising/tables/synthesis and named Zen intra-edge/restoration
  experiments are wired in their documented envelopes. Experiments stay opt-in.
- Wider chroma and 12-bit are rejected by C SVT and this backend. They are useful
  alternate-backend work, not missing shipping-C translations.

At implementation main **`0cbd1279`**, the latest native workspace gate passed
**2631/2631 tests, zero skips**. PR #20 uses published archmage/magetypes
**0.9.29**; 19 explicitly selected ARM SAD/variance tests passed under QEMU.
[Verification and pre-existing ARM Clippy/MSRV debt](rust/benchmarks/arm_pairwise_release_2026-09-08.md).
These local checks are not a claim of a new CI run.

The preceding eight-bit landing matrix passed **1100/1100** in its named
reference envelope. **Four native10 parity cells remain open**; their streams
decode but differ from C. See [identity status](rust/docs/IDENTITY-STATUS.md).
HDR MODE=ON has a standing 10-bit gate; older 8-bit 48/48 prose is historical,
without a corresponding retained standing gate. Per-reference, per-ISA and
corpus boundaries matter. No universal C parity or calibrated RD/time routing
is claimed.

## Use

The crates are not published to crates.io. Pin a reviewed Git revision:

```toml
[dependencies]
svtav1 = { package = "zenav1-svt", git = "https://github.com/imazen/zenav1-svt", rev = "0cbd1279f1e9e70ee62e051e56315e10f8b7f969" }
```

```rust
use svtav1::avif::AvifEncoder;
let pixels = vec![128u8; 16 * 16];
let encoded = AvifEncoder::new().with_quality(80.0).with_speed(6)
    .encode_y8(&pixels, 16, 16, 16).unwrap();
// encoded.data is an AV1 OBU sequence, not an AVIF container.
```

Use zenavif for RGB/RGBA conversion, AVIF still muxing and backend routing.
The optional `avif-container` feature provides the all-intra animation API.
Raw native-u16 inputs are available through `EncodePipeline`'s HBD methods;
do not infer a Gray16 zenavif entry point from that raw support.

## Develop

Read [the working guide](rust/docs/WORKING-ON-THIS.md) and [Rust rules](rust/CLAUDE.md).
From `rust/`, run `cargo nextest run --workspace --locked` under the shared
heavy-job wrapper. C-oracle tests need the `reference/svt-av1` submodule and
C build tools; product consumers do not. Mainline and HDR oracle builds are
owned by the dev-only `zenav1-svt-cref` build script. Fresh-machine timing and
portability acceptance remains tracked in issue #4.

[Package/source map](PORTING.md) · [full policy goal](ENCODER-POLICY-GOAL.md) ·
[documentation index and historical records](rust/docs/DOCUMENTATION-INDEX.md).

## License

The Rust port (`rust/`, and everything outside the submodule) is dual-licensed
**AGPL-3.0-only OR a commercial license** — the standard Imazen "zen" model
(same as zenavif et al.): [LICENSE-AGPL3](LICENSE-AGPL3) /
[LICENSE-COMMERCIAL](LICENSE-COMMERCIAL). Use it under the AGPL, or
[contact Imazen](https://imazen.io) for a commercial license.

**If someone covers Imazen's 2026 AI + server costs, we'll release the port
under MIT or the original upstream license.**

The SVT-AV1 **C tree** (the `reference/svt-av1` submodule) keeps its upstream
licensing: BSD-3-Clause-Clear plus the Alliance for Open Media Patent License
1.0 — see `LICENSE.md` / `PATENTS.md` *inside the submodule*. The Rust port is
a derivative work of that BSD-licensed C source; its upstream attribution and
patent terms are preserved, and relicensing the derivative is permitted by
BSD-3-Clause-Clear.

## Acknowledgments

- [SVT-AV1](https://gitlab.com/AOMediaCodec/SVT-AV1) (Intel / Alliance for Open
  Media) — the battle-tested C encoder this port is built on
- [svt-av1-hdr](https://github.com/juliobbv-p/svt-av1-hdr) (juliobbv-p) — the
  perceptual/HDR feature set ported in fork mode
- [rav1d](https://github.com/memorysafety/rav1d) — safe Rust AV1 decoder
- [archmage](https://github.com/imazen/archmage) — safe SIMD dispatch via CPU
  feature tokens

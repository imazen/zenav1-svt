# API and support audit, 2026-09-08

This is the current support index. Earlier dated port maps retain their original
measurements; their old gap lists are not the current implementation inventory.
Support means the setting reaches the implementation. It does not certify C
byte parity outside the cited gate envelope.

## Corrected API behavior

`AvifEncoder::validate_configuration_for_input(width, height, StillInputFormat)`
checks the actual monochrome or 4:2:0 entry point without allocating pixels.
The real encode methods use the same check. Configuration queries now validate
film-grain strength/tables and reference restrictions. Mono rejects grain,
mainline/parity references and color-only Zen enhancements before encoding.

The legacy `Encoder::send_frame` previously discarded frames while returning
success and incrementing its counter. It now returns an explicit unimplemented
error, including on flush. `receive_packet` returns the same permanent refusal
instead of inviting infinite `NotReady` polling. This corrects the API's report;
it does not implement public streaming video.

## Source and enabled-evidence inventory

| Area | C SVT v4.2.0 | Rust SVT / caller | Evidence and remaining boundary |
|---|---|---|---|
| 8/10-bit 4:2:0 | Accepted | Raw pipeline, AvifEncoder, zenavif RGB/RGBA | Current eight-bit landing matrix 1100/1100; four native10 cells remain explicitly open in `deferred-native10-parity.json` |
| 12-bit, 4:2:2, 4:4:4 | Rejected at C settings validation | Rejected by SVT | Extensions belong to alternate backends; not missing shipping C ports |
| Monochrome / alpha | No C monochrome encode | Rust extension; raw 8/10-bit, zenavif Gray8 and RGB/RGBA alpha | Odd-frame reconstruction, native/lossless tests; pristine/parity mono refused |
| Native research -1 | Accepted | Checked signed preset reaches real search | Reference-specific research gates and still_policy tests; no claim that all optional combinations are covered |
| Strict reference selection | Mainline and hybrid are distinct sources | Explicit reference and SvtParity policy | Pristine reference audit identifies the branch difference and scoped native8/10/-1 gates |
| Screen-content control 0/1/3 | Supported | Live forced classes for 0/1; detector for 3 | `pipeline.rs` sc_derivation; regression cells scm0-screen-p6 and scm1-gradient-p6/p8; old issue #17 gap text is stale |
| Mainline tune-0 sharpness | Supported | Live on the mainline key-frame path | `lf_sharp_eff`; tune0-mainline-sharpness regression. The old fork-only gate is gone |
| QM and variance boost | Supported | AvifEncoder and pipeline; zenavif SvtParams | Existing enabled tests; use backend-specific controls, not zenravif-only controls that another backend would ignore |
| Film grain model / denoiser / FFT / synthesis | Supported | C translation, pipeline lifecycle, AvifEncoder; now zenavif primary-color controls | `film-grain-port-map.md`: historical 28/28 valid C streams, 29/29 decoder reconstructions, enabled unit/production tests. New query/encode, wrapper/replay and displayed-grain tests. Wider inter/GOP and ARM NaN-conversion oracle remain separate |
| HDR-fork tools | Fork-specific | HdrForkConfig and its consumers; mainline reference rejects fork-only fields | `hdr_mode.rs`, reference validation and enabled HDR gates; wrapper exposes a subset, not the whole raw config |
| Static HDR and color metadata | Supported | AVIF wrapper/container paths | CICP, CLLI, mastering display, ICC/Exif/XMP tests. Wrapper audit preserves transfer codes and independent ICC+nclx |
| Superres | C accepts wider combinations | Restricted Rust pipeline envelope | `superres_config_error`: native10 source scaling and active restoration combinations remain unimplemented; no zenavif still control |
| Animated AVIF | Container feature | Working all-intra color/alpha/timing/metadata APIs | Animation gates are separate from the legacy streaming-video scaffold |
| Inter / reference/GOP / temporal / rate-control combinations | C supports video | Experimental pipeline has explicit narrower guards | Still support reports do not advertise general video support; public streaming video remains unimplemented |
| Zen intra-edge / restoration-unit search | Not parity behavior | Opt-in, wired and preserved | Enhancement and partial-frame decoder tests; no automatic RD-optimal bundle |

## API verification

- SVT workspace: 2631/2631 tests, zero skipped, including the support-query
  versus actual encode matrix and the streaming API refusal regression.
- SVT regression spotcheck: 139/139. The last streaming-scaffold change affects
  no pipeline path; its regression is covered by the subsequent workspace run.
- `tools/refusal_inventory.sh` regenerated its source-derived inventory.
- No new mode-decision, quantizer or entropy algorithm was changed by this audit.

Local logs: `~/tmp/svt-tracking/api-support-final-nextest.log` and
`api-support-spotcheck.log`. Existing stream/trace archives are retained via
`native10-handoff-receipt.json`; deferred cells are not marked passing.

## Required follow-ups

Complete the four native10 parity fixes, supported-C video/temporal/reference
and superres combinations, and the remaining HDR-fork wrapper surface. Finish
broad corpus/held-out calibration before enabling fractional adaptive search or
measured backend-optimal routing. These are implementation/calibration work,
not completed by support queries or by the API audit.

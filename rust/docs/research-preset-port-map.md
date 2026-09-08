# Native research preset -1: live wiring audit

Work in progress, 2026-09-08. Native -1 is not yet a verified parity mode.
The next effort region is the Zen-only AOM-technique continuation specified in
[the encoder policy goal](../../ENCODER-POLICY-GOAL.md), after native -1 works.

Reference inspected: `reference/svt-av1` commit
`3115c0c1b23e860dfd75c94f6740e0298182dd13`. The reference tree contains the
conditional HDR hybrid; record its build mode as well as this source identity.

## Transport and current validation

`speed_config::NativePreset` checks -1 through 13. `EncodePipeline::new_with_preset`
and the identity example carry it without unsigned wrapping. The legacy
`EncodePipeline::new` and `SpeedConfig::from_preset` retain unsigned entry points.
Internal mode/partition/filter/search derivations now carry signed presets.
This does not establish that every research-only consumer is implemented.

The first signed transport revision passed all 2,612 workspace tests, including
two new representation/SGR-control checks. Those new checks are transcription
and transport checks, not C output comparisons. Before changes, the baseline
was 2,610 tests and 127/127 regression spot-checks. The signed revision also
passed 127/127 regression spot-checks; no research identity claim follows from
that normal-preset suite.

## Required research-mode differences

| Area | Verified C behavior | Live port status |
|---|---|---|
| SGR restoration | `get_sg_filter_level_allintra` returns level 1 at -1: both lanes search all 16 entries with refinement | Signed selector now reaches all-intra search controls; full enabled-output validation pending |
| Wiener restoration | All-intra -1 and 0 both use level 3 | Existing controls retained |
| CDEF | Research level differs from normal preset 0 | Confirmed live omission: 8/10-bit search loops only compute first-pass UV rows, despite level 1 enabling both passes. Fix the per-slot chroma mask before the research identity matrix |
| Depth refinement | All-intra screen level 1; nonscreen research level 3 instead of level 6 | Signed research branches added to the live depth configuration |
| Partition geometry | `get_nsq_geom_level_allintra(-1)` selects level 1, enabling HA/HB/VA/VB | HA/HB/VA/VB generation, pruning, first-child reuse and picture coefficient-class transport added; geometry/unit tests and normal byte regressions pass. Chosen-asymmetric C-output and independent-decoder witnesses pending |
| Partition search | Low/VLow coefficients reduce the research NSQ search level | Frame coefficient class now travels through `CodingQuantCfg` to all three pipeline `NsqCfg` consumers; research Low/VLow branches are reached |
| Lambda weighting | At -1, tune 0–2 omit the normal QP-dependent weight; IQ curve and extended-CRF bump still apply | Signed `frame_lambda_weight_for_preset` now reaches main and per-SB consumers; the tile override preserves zero research weight instead of falling back to normal defaults. 2,613 workspace tests and 127/127 normal-preset byte regressions pass; enabled research output gate pending |
| Video derivations | Research branches affect additional prediction/search tools | Signed carriers alone are insufficient; audit the full default arm before video parity claims |
| Public wrappers/query | Research must be reachable and report verified support | High-level AVIF speed mapping remains unchanged; research wrapper support is pending full implementation |

Do not alias research to preset 0, suppress its asymmetric shapes, weaken an
identity expectation, or label this table complete based on accepting -1.
Complete translation and wiring first, then run the enabled research identity
and independent decoder matrix. Normal preset regression gates remain required
throughout the implementation.

## Corpus reduction prerequisite

The canonical repository is [imazen/imazen-26](https://github.com/imazen/imazen-26).
Its [access index](https://github.com/imazen/imazen-26/blob/main/ACCESS.md) points
to authoritative split manifests with render URLs; render names cannot safely
be inferred from original names because EXIF rotation changes dimensions.
Read and pin the manifests and variant registry before declaring scouting jobs.
Existing K-sized diversity-cluster subsets are not the requested minimum
RD/RD-speed-zone set. The selection and validation requirements are in section 5
of the policy goal. No reduced RD-zone set has been measured yet.

Canonical metadata snapshot: commit `187fbf338ce08e8e6654db7f04ddae58d5263da2`,
local `/home/lilith/tmp/svt-tracking/imazen26-canonical/provenance.json` with file
hashes. Parsed coverage: train 1,084 origins (1,082 SDR URLs, 38 HDR URLs),
validate 658 (657 SDR, 20 HDR), test 418 (418 SDR, 18 HDR). Missing SDR renders
are raw DNG origins 1444/1458 (train) and 1455 (validate); do not silently omit
them from a full-corpus claim. URL presence is not a download/decode check.
The split documentation also identifies related patent scans, brochures and
website viewports across IDs; account for that leakage when fitting policies.

Additional source-audit finding: the C video default arm assigns lambda weight
300 to non-I frames at QP >= 62 (normal presets), while the shared Rust helper
currently assigns 175. Preserve this as a separate enabled-witness investigation;
research mode correctly omits both normal weights.

Asymmetric-shape implementation now follows the whole decision path: C iteration
order is N/H/V/H4/V4/HA/HB/VA/VB; incomplete blocks inject only H/V, size 8
excludes the asymmetric shapes, and size 128 excludes H4/V4. Besides geometry,
the port now carries the HA/HB/VA/VB arms of reconstruction/transform
pruning from `product_coding_loop.c`, `update_skip_nsq_shapes` (-10 coefficient-free aggressive offset),
and `update_redundant` (HB←H, VB←V, VA←HA first-child reuse). Merely generating
the three children is not full C search parity. The existing packer already
has asymmetric partition context/offset arms; validate those with chosen
asymmetric blocks rather than assuming source presence is sufficient.

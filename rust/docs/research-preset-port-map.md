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
| CDEF | Research level differs from normal preset 0 | Existing signed level helper is reached; audit all search consumers and chroma passes |
| Depth refinement | All-intra screen level 1; nonscreen research level 3 instead of level 6 | Signed research branches added to the live depth configuration |
| Partition geometry | `get_nsq_geom_level_allintra(-1)` selects level 1, enabling HA/HB/VA/VB | **Missing:** `depth_refine::shapes_for_size` and `shape_children` only generate N/H/V/H4/V4; trace geometry, search, entropy costs, coding and reconstruction before claiming support |
| Partition search | Low/VLow coefficients reduce the research NSQ search level | **Missing wiring:** `part_arm::nsq_search_level` still passes Normal; carry the real frame coefficient class to all `NsqCfg` consumers |
| Lambda weighting | At -1, tune 0–2 omit the normal QP-dependent weight; IQ curve and extended-CRF bump still apply | **Missing wiring:** `pd0::frame_lambda_weight` has no preset argument; update all main and per-SB consumers consistently |
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

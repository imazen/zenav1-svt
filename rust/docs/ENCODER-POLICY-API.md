# Encoder policy and routing contract v1

User priority, 2026-09-08: land verified work, then settle and wire API/routing.
Known parity cells may remain explicitly tracked; no feature or experiment is
abandoned. This contract separates interface stability from calibration coverage.

## Ordinary request

The ordinary caller sets quality, a checked `Effort` in [0, 1], and policy.
Effort is an ordinal work preference, not seconds, a preset number, or a promise
that adjacent values produce different searches. Reject NaN, infinities and
out-of-range values. Preserve existing speed/native-preset entry points and
legacy defaults. A resolved plan reports the actual native preset and effective
work policy; a native bucket is labelled as such, not described as fractional
adaptive search. Adaptive fractions and the Zen continuation beyond native -1
remain required work, with versioned, measured resolutions rather than invented
preset interpolation.

Policy is `SvtParity(reference)` or `Zen`. The default reference for a newly
requested parity policy is pristine mainline 4.2.0. Legacy constructors retain
their existing reference unless policy is explicitly set. Parity forbids Zen
experiments, hybrid-only controls on mainline, alternate-backend routing and
Rust-only format extensions. A policy constrains decisions; it does not erase
the tracked implementation divergences or certify untested configurations.

Backend selection is separate: explicit selection retains its failure contract;
automatic selection may choose among eligible backends. Explicit SVT controls
cannot migrate to an encoder that ignores them. No encoder-error fallback.

## Query, resolve, execute

1. Describe actual input kind (RGB8, RGBA8, RGB16, RGBA16 or mono), dimensions,
   requested coded precision/chroma, color/range, alpha semantics, losslessness,
   metadata and explicit tool requirements. Alpha presence and grayscale are
   distinct facts. Never infer wrapper support from raw codec support.
2. Query each built backend's shared production validator. Intersect with the
   actual zenavif entry point/muxer's support. Unbuilt backends report a refusal.
   No allocations or trial encodes are needed for configuration support.
3. Keep suitability separate from support. A known estimate includes metric,
   expected bytes/quality/time, calibration/build revision, input envelope,
   hardware cohort and uncertainty. Missing estimates remain unknown; they
   must never be represented by zero duration, a made-up preset score, or an
   unlabelled hardcoded crossover. Backend implementations own these estimates.
4. For automatic routing, rank supported candidates by their reported measured
   tradeoff for the requested preference only inside the calibration envelope.
   In unmeasured regions retain the requested preferred backend when eligible;
   otherwise select a supported alternative deterministically and label the
   reason as capability availability, not measured optimality. Source-conditioned
   routing models require full-corpus/held-out evidence before activation.
5. Produce an inspectable resolved route containing input requirements, selected
   backend/settings, reference, enhancement set, resolution/schema revision,
   selection reason, candidate refusals, estimates and calibration identity.
   Execute that exact resolved config through the existing public encoder seam.
   Revalidate replay against the actual input and available backend versions.
6. Stable serialized replay and cache identities include effective configuration,
   backend implementation revision, reference, policy, adaptation/calibration
   revision and requested pixel semantics. Host-dependent thread choices must
   be resolved or explicitly marked nonportable. Existing encode-plan/tuner
   machinery is reused rather than duplicated into another ranking system.

## Delivery and follow-ups

The first execution router must make real public RGB/RGBA entry points select
and execute supported alternatives, and preserve explicit-backend behavior.
Test query/encode agreement and unchanged format/metadata/alpha semantics,
feature-off refusals, unsupported-all errors and explicit parity restrictions.
Monochrome must have its own truthful input-kind handling; it must not be passed
as RGB to a query. A source-buffer or encoder failure is propagated unchanged.

Backend suitability starts unknown wherever broad evidence is missing. Complete
calibration, adaptive fractional budgets, additional formats and optimality
claims are tracked follow-ups, not API placeholders described as implemented.
AOM ownership and its PR stack remain in imazen/zenav1-aom#16. The four SVT
native10 cells remain in deferred-native10-parity.json, with retained hashes,
fixtures and the unproven precision experiment available in handoff history.

Landing-base verification: 2627/2627 workspace tests (zero skipped), 139/139
regression witnesses, 1100/1100 eight-bit identity cells (zero pins/errors).
All four stored native10 C/Rust pairs independently decode with aomdec; their
byte-parity differences remain explicitly open. Full logs are preserved under
`~/tmp/svt-tracking/policy-landing-*`. No calibration completeness is claimed.

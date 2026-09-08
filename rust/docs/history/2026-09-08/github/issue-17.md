# Historical GitHub issue #17: tune=0 and screen_content_mode=3 are no-ops: byte-identical bitstreams vs baseline (288/288 cells, presets 4+6)

Snapshot before the 2026-09-08 handoff cleanup. State at capture: **OPEN**.
Original: https://github.com/imazen/zenav1-svt/issues/17. Current disposition: [issue audit](../../../OPEN-ISSUES-AUDIT-2026-09-08.md).

## Original body

Two encoder knobs accepted by the SVT port produce **byte-identical bitstreams to
baseline** on every cell of a 49,120-cell AVIF DOE wave: `tune = 0` and
`screen_content_mode = Some(3)`. `tune = 3` *does* move bytes, so the tune
plumbing is partially wired rather than absent.

Filing from the zenmetrics side, where this was measured. Everything below is
byte equality on encoder output — no modelling, no metric, no judgement call.

## What was measured

For each single-deviation arm, the fraction of its `(image, q)` cells whose
encoded bitstream is byte-identical to the default arm at the same
`(image, speed, q)`:

| knob | setting | identical @ preset 4 | identical @ preset 6 |
|---|---|--:|--:|
| **`scm3`** | `screen_content_mode = Some(3)` | **288/288 = 100%** | **288/288 = 100%** |
| **`tn0`** | `tune = 0` (VQ) | **288/288 = 100%** | **288/288 = 100%** |
| `mtx32` | `max_tx_size = 32` | 72/288 = 25.0% | 79/288 = 27.4% |
| `qml1.8.15` | QM window (8,15) | 32/288 = 11.1% | 32/288 = 11.1% |
| `acb1` | `ac_bias = 1.0` | 17/288 = 5.9% | 37/288 = 12.9% |
| `acb3` | `ac_bias = 3.0` | 16/288 = 5.6% | 24/288 = 8.3% |
| `shp3` / `shp7` | `sharpness` 3 / 7 | 5.6% / 5.2% | 0% / 0% |
| `bd10`, `qml1.2.10`, `qml1.4.10`, `tl1.0`, `tl1.1`, `tn3`, `vbst×3` | — | 0% | 0% |

Every other knob moves the bitstream on most or all cells. These two move it on
none — at every image, every quality point, and both presets.

## Minimal repro

Image `7004.scale1024x1024.png`, q 45, speed 4, plan `svt_doe_main`, corpus
`avif-doe-1024-2026-09-01`:

| arm | `encode_sha` | bytes |
|---|---|--:|
| `s4-svt-420` (default) | `e07ecbc4f28e4339…` | 52,306 |
| `s4-svt-420-tn0` | `e07ecbc4f28e4339…` | 52,306 ← **same bitstream** |
| `s4-svt-420-scm3` | `e07ecbc4f28e4339…` | 52,306 ← **same bitstream** |
| `s4-svt-420-tn3` | `24cfb87a676e5b69…` | 53,709 ← tune *does* work at 3 |

This is not the sweep planner collapsing aliased configurations before encode.
The harness's resolved-state fingerprint separates the arms — it believed it was
varying something and encoded each one:

| arm | fp @ q5 | fp @ q45 | fp @ q96 |
|---|---|---|---|
| `s4-svt-420` | `39121e954d30bfe5` | `525f02196fc98147` | `27ef653437e59dc7` |
| `s4-svt-420-tn0` | `ee1082fec17e38e5` | `98b717369c47c847` | `c19e9bac140044c7` |
| `s4-svt-420-scm3` | `edf9fd1ddcd7f88b` | `66969d4a0b2866c1` | `bc0f079c597a1b41` |
| `s4-svt-420-tn3` | `cd9ed696474abe9a` | `a3567e1386ad8a08` | `d3624754e847ea88` |

## A lead, offered as hypothesis rather than diagnosis

Read from the consumer side (`zenavif`, which drives this port) — not verified
against the port's own source, so please treat as a starting point:

- `zenavif/src/encoder_svt_rs.rs:269` forwards `pipeline.hdr.tune = p.tune;` and
  `:279` forwards `pipeline.hdr.screen_content_mode = p.force_screen_content_mode;`,
  so both values do reach the port's config.
- `zenavif/src/expert.rs:258-266` documents that tune 3 (IQ) and 4 (MS-SSIM)
  rewrite other config fields at encode time via the port's
  `HdrForkConfig::apply_tune_overrides` (`enable_qm`, QM levels, `sharpness`, the
  variance-boost trio, and for IQ `max_tx_size` and `screen_content_mode`).
  Tunes 0 and 1 rewrite nothing.

That fits the measurement exactly: if `tune` influences the bitstream *only*
through `apply_tune_overrides`, then tune 0 and the default tune 1 necessarily
resolve to identical encoder state, while tune 3 differs because the override
changed other fields. The question for the port is whether anything downstream
of config resolution reads `tune` directly (mode-decision RD weighting, etc.).

For `screen_content_mode`, `Some(3)` is the value that should force the
anti-alias-aware detector on (palette + IntraBC) regardless of preset. Worth
checking whether an out-of-range guard clamps or drops it on this path.

One testing note that may matter more than either knob: the existing parity test
`resolved_matches_the_port_tune_overrides`
(`zenavif/src/encoder_svt_rs.rs:1260`) compares our resolved config against the
port's `apply_tune_overrides` for tune 0..4 — including `screen_content_mode`.
It passes. It compares **config to config**, so it cannot detect a field that is
resolved correctly and then never consumed by the encoder core. A bitstream-level
assertion (two configs differing in one field must not produce identical bytes,
for the fields where that is guaranteed) would have caught both of these.

## Why it is worth fixing rather than documenting

Because the two arms are exact no-ops, every DOE pair containing one is a
byte-identical alias of the other knob's single arm. Verified, not inferred —
all **27** such pairs are identical on 288/288 cells each, and `tn0-scm3` is an
alias of the bare control:

```
s6-svt-420-tn0-mtx32      ==  s6-svt-420-mtx32       288/288
s6-svt-420-scm3-qml1.2.10 ==  s6-svt-420-qml1.2.10   288/288
s6-svt-420-tn0-scm3       ==  s6-svt-420             288/288
   … 27 of 27 pairs, all exact
```

Cost in the wave that found it: **8,972 cells, 21.8%** of the A1+A2+AG blocks,
and **29 of A2's 118 strata (24.6%) carry no information**. Any future sweep that
includes these knobs pays the same tax and, worse, reports a residual of exactly
zero as though it were a measured null result.

To be clear about what this is not: it is not a correctness bug. Every bitstream
produced is valid and decodes correctly. It is a knob that silently does nothing.

## Evidence

- Full writeup: `zenmetrics/benchmarks/avif_doe_stageA_2026-09-02.md` §3
- Per-arm byte identity: `arm_byte_identity.tsv` under
  `/mnt/v/output/zensim-avifdoe/stagea_a0r/`
- Scored dataset (49,120 × 18):
  `/mnt/v/output/zensim-avifdoe/doe_scored_2026-09-02.parquet`
- Pointer + shas: `zenmetrics/benchmarks/avif_doe_stageA_2026-09-02.pointer.md`
- Declaration: `zenmetrics/scripts/jobsys/avifdoe_declare.sh`

Wave conditions: presets 4 and 6, 32 images, 9-point q ladder, zero encode
failures across all 49,120 cells.


## Historical comment — 2026-09-02T14:30:35Z

## Recheck 2026-09-02: `tune=0` NOT fixed; `scm=3` reclassified as correct-by-identity; a third arm (`scm=0`) is genuinely unplumbed

Re-probed at **`0284b855c`** (308 commits after the `30cf4b3d0` the DOE encoder was
built from, including `188948556` sc-detector tier-1, `42da724da` intra-BC video arm,
`b8e5e1c11` tune-vmaf). Verdicts differ per knob, so the original issue splits.

### Method

Probe at the **port boundary**, not through the consumer: `EncodePipeline::new(w, h,
preset, Cqp, 0, 1).with_chroma_420(true)`, `hdr = HdrForkConfig::mainline()`, then
**exactly one field** overwritten — the construction `zenavif::encoder_svt_rs` uses
(`encoder_svt_rs.rs:690` + `apply_svt_params`, pinned by its own
`svt_params_default_leaves_the_pipeline_at_mainline`).

288 encodes: 3 content classes (photo / **screen** / detail) × presets **4, 6, 8** ×
qp 20/32/45/55 × 8 arms. Committed as
`rust/svtav1/examples/knob_byte_identity.rs` — this is the bitstream-level assertion
the issue asked for.

**Positive controls are the point.** A null is worthless if the probe is blind:

| arm | identical / total | reads |
|---|--:|---|
| `CTRL_tn3` (`tune=3`) | **0/36** | probe sees `tune` |
| `CTRL_shp7` (`sharpness=7`) | **0/36** | probe sees `sharpness` (and same-length byte changes) |

### 1. `tune = 0` — STILL A NO-OP. Not fixed. Root cause localised.

| arm | p4 | p6 | p8 | total |
|---|--:|--:|--:|--:|
| `tn0` (mainline) | 12/12 | 12/12 | 12/12 | **36/36 identical** |
| `fork_tn0` (same delta, `hdr_fork()`) | 4/12 | 0/12 | 0/12 | **4/36 identical** |

Those two rows differ in **one predicate**. `pipeline.rs`:

```rust
let lf_sharp_eff: u8 = {
    let base = self.hdr.sharpness.clamp(0, 7) as u8;
    if self.hdr.is_fork() {
        crate::tune::lf_sharpness_for_tune(base, self.hdr.tune, base_qindex)
    } else {
        base            // <-- mainline: the per-tune ladder is never consulted
    }
};
```

`lf_sharpness_for_tune` is the **only** site that can separate tune 0 from tune 1.
I walked every `tune` read in `svtav1-encoder`; each other one keys on `TUNE_IQ`,
`TUNE_MS_SSIM`, or `tune_uses_ssim_rdmult` (`SSIM|IQ|MS_SSIM`) — all of which exclude
both 0 and 1:

`hdr_mode.rs:358,369` · `chroma_q.rs:89` (`mainline_chroma_q_deltas`) ·
`chroma_q.rs:122` (`tune_chroma_boost`, VQ→0) · `pipeline.rs:2322,2382,2546,2578,2977,2985,10020` ·
`pd0.rs:252` (`frame_lambda_weight(_, tune_iq: bool, _)`) · `md_config.rs:927` ·
`mds3.rs:2843` (`frame.tune_ssim`).

So in mainline mode `tune ∈ {0,1}` is **structurally** one configuration.

Note the shape of the fork arm's differences: `photo p4 q20` goes 2846 B → 2846 B with
a different hash. Same length, different bytes — the signature of a deblocking-strength
change, which is exactly what `TUNE_VQ ⇒ min(7, sharpness+2)` should produce.

**The open question is for the port program, not for me to answer from here:** the
adjacent `apply_tune_overrides` call site carries the comment *"These four (tune, QM,
variance boost, sharpness) are MAINLINE v4.2.0 features, not fork additions — they used
to be gated behind `is_fork()` here, which silently ignored them in mainline mode."*
That is the same gate, one screen away, on the one remaining tune consumer. Whether
`deblocking_filter.c:1157`'s VQ arm is mainline v4.2.0 or hybrid-only decides whether
the fix is "drop the `is_fork()` gate" or "document tune 0 ≡ tune 1 as faithful". I
could not settle it — the `reference/svt-av1` submodule is not checked out locally, so
this is a lead with a measurement behind it, not a diagnosis.

### 2. `screen_content_mode = Some(3)` — NOT A BUG. My original filing was wrong.

It **does** reach the bitstream. It is inert at presets ≤ 7 *because C's own allintra
default is already 3 there*:

- `sc_detect.rs:497` — `ScArm::Allintra => preset <= 7`
- `pipeline.rs` — `Some(3) => self.speed_config.preset.min(7)`, the identity at 4 and 6

At preset **8**, where `min(7)` finally bites, it moves bytes hard on screen content:

| cell | base | scm3 |
|---|--:|--:|
| screen p8 q20 | 2,898 B `643fae26…` | **930 B** `93ea00a9…` |
| screen p8 q32 | 2,186 B | **833 B** |
| screen p8 q45 | 1,615 B | **833 B** |
| screen p8 q55 | 1,984 B | **807 B** |

Photo and detail stay identical at p8 — the detector correctly finds no screen content.
**The DOE swept presets 4 and 6 only, which is precisely the range where `scm=3` is a
semantic identity.** So the wasted cells are real, but the cause is the sweep design,
not the port. `scm3` is a live knob for any Stage-B stratum at preset ≥ 8.

### 3. NEW: `screen_content_mode = Some(0)` **is** unplumbed — at every preset

`scm0`: **36/36 byte-identical**, including preset 8. The match arm is
`Some(3) => preset.min(7), _ => preset`, so `Some(0)` falls through the wildcard and is
indistinguishable from `None`. At presets ≤ 7 it should *disable* screen-content tools
and does not. Not swept by the DOE, so it cost nothing — but it is the real instance of
the bug class this issue was opened for.

### Minimal repro (screen, p6, q45) — one sha256 for four "different" configs

```
base   851 B  3a7e704c30f141c9699fe32110bed8ed19c5d216404641ea541e06f914f0d60c
tn0    851 B  3a7e704c30f141c9699fe32110bed8ed19c5d216404641ea541e06f914f0d60c
scm3   851 B  3a7e704c30f141c9699fe32110bed8ed19c5d216404641ea541e06f914f0d60c
scm0   851 B  3a7e704c30f141c9699fe32110bed8ed19c5d216404641ea541e06f914f0d60c
tn3    854 B  (differs)
shp7   851 B  (differs — same length, different bytes)
```

Reproduce: `cargo run --release -p zenav1-svt --example knob_byte_identity -- <outdir>`

### Consequences for the consumer

- **`tn0` stays dead** for Stage B until the gate question is settled. No rerun declared.
- **`scm3` is not dead** — it is a preset-8+ knob. Re-including it at presets 4/6 would
  repeat the tax; including it at 8+ measures something real.
- The 27 aliased A2 arm-pairs and 8,972 cells in the original report stand **exactly as
  measured** for the `30cf4b3d0`-era binary. Nothing there is retracted.

Keeping this open on the `tune=0` gate and the `scm=0` wildcard.


## Historical comment — 2026-09-02T14:34:09Z

Follow-up: the probe landed as `70883fbe8` (`rust/svtav1/examples/knob_byte_identity.rs`) and the 288-cell TSV is **byte-identical** re-run at `6fe01232` (two commits later, one of them `ac402f05f` touching PD0 rate tables). So the verdict above is current at HEAD, not just at `0284b855c`.

```
cargo run --release -p zenav1-svt --example knob_byte_identity -- <outdir>
```

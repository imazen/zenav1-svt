# Practical usage plan — what real SVT-AV1 AVIF/still encoding looks like, and how the port should prioritize

Status: research note (2026-07-19). READ-ONLY analysis; additive. Written to reprioritize
the byte-identity roadmap against *real-world* SVT-AV1 still-image usage. Every external claim
is cited with a URL. Where mainline default, quality-community recommendation, and actual-pipeline
behavior differ, they are labelled separately — they are **not** interchangeable.

> **Concurrency note:** written while a concurrent session (`claude-bd10-md`) held the repo
> `.workongoing` marker for true-bd10-MD work. This file is a brand-new doc that touches no code
> and no existing file, so it cannot conflict with that work; the marker was not overridden.

---

## TL;DR (the headline answers)

1. **Is 10-bit the practical default for quality AV1 stills/AVIF? — YES, for the quality path
   this port serves.** The AVIF quality community recommends encoding at **10-bit even from an
   8-bit source**, because AV1's internal precision makes 10-bit *more* coding-efficient (fewer
   rounding errors, better gradients, no banding) at a modest size cost. 8-bit still dominates by
   raw volume in commodity web tooling (sharp/libvips default to 8-bit — a build constraint, not a
   quality choice), but zenavif/imageflow compete on the **quality** axis, where 10-bit is the
   default. **The port should treat bd10 4:2:0 as a co-primary envelope, not a secondary corner.**

2. **Chroma: SVT-AV1 is functionally 4:2:0-only.** Mainline's own `--color-format` doc says
   *"only yuv420 is supported at this time"* — the 4:2:2/4:4:4 enum values exist but are
   non-functional. So **4:2:0 is the correct and only real chroma target for an SVT-AV1 port.**
   The quality community's "use 4:4:4 for AVIF" advice explicitly means *use aomenc, not SVT-AV1*.
   → The port's current 4:2:0 focus is **right**; the `4:4:4 / 4:2:2` items in ACCEPTANCE-CRITERIA
   are **not real mainline SVT-AV1 configs** and should be de-scoped or marked "blocked on upstream."

3. **Preset reality: quality stills use M2–M6; fast/thumbnail stills use M8–M13.** M0/speed-0 is a
   strict-parity anchor, **not** a common real preset. The port already sweeps M0/2/3/6/10/13, so
   preset *coverage* is fine — the reprioritization is about **bit depth**, not preset.

4. **The single highest-value gap is bd10 real-content mode decision** — the port's 10-bit path is
   byte-exact only on flat/uniform + a narrow DC-family/tx_depth-0 subset today; directional /
   filter-intra / tx_depth>0 / chroma bd10 frames fall back to the u8 output. Since 10-bit *is* the
   quality-AVIF default, closing that gap is the most practically-useful next work. This aligns with
   the port's own stated P0 (10-bit + arbitrary dimensions) and sharpens it.

---

## 1. Bit depth — is 10-bit the real default for quality stills?

**Quality-community recommendation (strongest evidence): 10-bit, even from 8-bit sources.**

- Codec Wiki's AVIF guide states plainly: *"Specifying a value below 10 isn't recommended, as it
  will hurt coding efficiency even with an 8 bit source image."* — i.e. set depth 10 regardless of
  source. Its canonical quality command is `avifenc -c aom -s 4 -j 8 -d 10 -y 444 ...`
  (`-d 10` = 10-bit). <https://codecs.wiki/docs/images/AVIF>
- **Why 10-bit is *more* efficient even for 8-bit content:** AV1 processes internally at ≥10-bit
  precision even for 8-bit output; encoding at 10-bit reduces rounding error through the transform/
  quant/prediction chain, which is reported to compress SDR content ~5–12% smaller at equal quality
  and to remove gradient banding (skies, sunsets, soft shadows). Treat the 5–12% as *reported, not
  measured here*. <https://www.forasoft.com/learn/video-encoding/articles/8bit-vs-10bit-encoding>
- **PSY/quality forks make 10-bit the default or mandatory:**
  - `svt-av1-hdr` (juliobbv-p) defaults to 10-bit and its whole design intent is
    "visually-optimal SDR and HDR image and video." <https://github.com/juliobbv-p/svt-av1-hdr/>
  - `SVT-AV1-Essential` (nekotrix) *forces* 10-bit (there is literally a discussion titled
    "How bad is patching out the mandatory 10-bit check?").
    <https://github.com/nekotrix/SVT-AV1-Essential/discussions/20>
  - SVT-AV1-PSY's `--hbd-mds` (high-bit-depth mode decision) forces HBD/10-bit mode decision at the
    top presets. <https://github.com/BlueSwordM/svt-av1-psyex>

**Mainline default (for contrast): 8-bit.** SVT-AV1's `--input-depth` is `[8, 10]`, default **8**,
and controls both input and output bitstream depth. 12-bit is unsupported.
<https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/master/Docs/Parameters.md>

**What commodity web pipelines actually do (for contrast): mostly 8-bit, by tooling constraint.**
sharp/libvips output 8-bit AVIF by default; 10-bit requires an HDR build of libvips, and 10-bit
support has been an open feature request. So the *volume* leader on the web is 8-bit — but that is a
packaging limitation, not a quality verdict.
<https://github.com/lovell/sharp/issues/4031> ·
<https://openaviffile.com/best-settings-for-avif-encoding/> (mainstream advice: "8-bit unless you
see banding; 10-bit helps gradients at +15–25% size").

**Verdict for the port:** the user's suspicion is **correct for this port's target**. zenavif/
imageflow are a *quality* AVIF path, and the quality path is 10-bit. bd8 remains high-volume and
must stay green as the anti-regression baseline, but **bd10 4:2:0 should be co-primary with bd8**,
not a corner.

---

## 2. Chroma — SVT-AV1 is 4:2:0-only in practice

- **Authoritative:** SVT-AV1 `--color-format` is `[0-3]` default `1` (yuv420) with the explicit note
  *"only yuv420 is supported at this time"* `[0: yuv400, 1: yuv420, 2: yuv422, 3: yuv444]`. The
  422/444 values are non-functional stubs.
  <https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/master/Docs/Parameters.md>
- **Community corroboration:** Codec Wiki: *"aomenc through avifenc is widely considered the best
  way to encode AVIF images, as SVT-AV1 only supports 4:2:0 chroma subsampling, rav1e isn't fast
  enough for still images, & the libaom team have put more effort into intra coding."*
  <https://codecs.wiki/docs/images/AVIF> · <https://codecs.wiki/docs/encoders/SVT-AV1>
- **AVIF quality practice uses 4:4:4 — but via aom, not SVT.** The canonical quality command is aom
  + `-y 444` + `-d 10`. 4:4:4 preserves sharp color edges (text, logos, screenshots); 4:2:0 is the
  photo/natural-image default and cuts size 30–40%.
  <https://codecs.wiki/docs/images/AVIF> · <https://openaviffile.com/best-settings-for-avif-encoding/>

**Verdict for the port:** 4:2:0 is the **only** real SVT-AV1 chroma target. Keep it primary.
**Monochrome (4:0:0)** is a legitimate grayscale-AVIF case and a real AV1 profile → modest priority.
**4:4:4 / 4:2:2 are not functional in mainline SVT-AV1** → the ACCEPTANCE-CRITERIA line "4:2:0 /
4:4:4 / monochrome" overstates reality; either de-scope 4:4:4/4:2:2 or annotate them "blocked on
upstream SVT-AV1 functional 4:4:4." If zenavif ever needs 4:4:4 stills, that is an **aom-backend**
job, not svtav1-rs.

---

## 3. Preset, CRF, tune, and the AVIF still-mode

**Presets (M0–M13).** Mainline `--preset` is `[-1..13]`, default **8**. Findings from Codec Wiki's
v3.0.x deep-dive series:
- **M2–M4 are the efficiency sweet spot** for quality; M2 ≈ M1 efficiency at ~2× the speed; M3 is an
  "awkward middle ground"; **M4** is "noticeably faster and reported to give higher-fidelity
  results." <https://codecs.wiki/blog/svt-av1-fourth-deep-dive-p1> ·
  <https://codecs.wiki/blog/svt-av1-fourth-deep-dive-p2>
- **M5–M8** are the usable faster tier; **M8 is "the last preset deemed truly usable"; M9–M13** show
  quality-consistency problems (fine for thumbnails/speed, not quality).
  <https://codecs.wiki/blog/svt-av1-fourth-deep-dive-p1>
- **Real still-mode recipes span the range:** quality AVIF uses **preset 2–6** with `--avif 1
  --tune 3 --crf ~30`; libheif's still example uses **preset 9** for cheap/fast stills.
  <https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/master/Docs/Parameters.md> ·
  <https://github.com/strukturag/libheif/issues/1636>
- **svt-av1-hdr changes the default preset to 4** (its stills-oriented default).
  <https://github.com/juliobbv-p/svt-av1-hdr/>

→ Real quality stills live in **M2–M6**; fast/thumbnail stills in **M8–M13**. **M0/speed-0 is a
parity anchor, rarely used in production.**

**CRF/QP.** Both default **35**; `--qp` is `[1..63]` integer, `--crf` is `[1..70]` in 0.25 steps.
<https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/master/Docs/Parameters.md>. Quality-still bands:
HQ ≈ CRF 10–23, general quality ≈ CRF 20–30, and SVT still-mode guides suggest starting at CRF 30;
SVT-AV1 "begins to struggle around CRF 38–44." <https://codecs.wiki/blog/svt-av1-fourth-deep-dive-p1>.
avifenc's aom quality example uses `cq-level=16` (high quality). <https://codecs.wiki/docs/images/AVIF>.
The web-aggressive band (CRF ~30–55) matters as much as the HQ band — do not over-weight high-CRF.

**Tune (a byte-parity axis — tune changes RD decisions, hence bytes).** Mainline `--tune` is
`[0..5]`, default **1**: `0=VQ, 1=PSNR (default), 2=SSIM, 3=IQ (still-image only), 4=MS-SSIM,
5=VMAF (video-only)`. <https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/master/Docs/Parameters.md>
- **Tune 3 (IQ) is the still-image tune** — content-detection + SSIM-RD changes for stills; the
  recommended tune for AVIF. Added in **SVT-AV1 4.0** alongside MS-SSIM and the AVIF mode.
  <http://aomedia.org/blog%20posts/SVT-AV1-4_0-Boasts-Major-API-Updates/> ·
  <https://www.phoronix.com/news/SVT-AV1-4.0>
- **Tune-number collisions across forks (parity hazard):** PSY-family forks remap tune numbers
  (PSY "Tune Still Picture" was `--tune 4`; PSY `--tune 3` = subjective SSIM; the fork's `--tune 5`
  = Film Grain vs mainline `5` = VMAF). `--tune N` is **not** portable across mainline vs fork.
  The port targets mainline v4.2 numbering; treat fork tune remaps as a gated-mode concern.
  <https://github.com/AOMediaCodec/libavif/issues/2412> · <https://halide.cx/blog/improving-avif-in-open-source/>

**AVIF still-mode (`--avif`).** Default **0**; `--avif 1` enables *"still-picture coding
optimizations for improved coding efficiency and reduced memory usage"* (fewer parallel picture
sets, a different rate-control mode). Real AVIF pipelines (libheif) use it.
<https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/master/Docs/Parameters.md> ·
<https://github.com/strukturag/libheif/issues/1636>. It changes encode structure → it is a real
**parity axis**, not just a memory optimization.

---

## 4. Forks / PSY — what actually matters, reconciled with the port's own pareto data

- The HDR/PSY family is a **tuning fork, not an architecture fork** — bitstream tools, partitioning,
  prediction, entropy are mainline; the deltas are *how decisions are weighted for HVS* (variance
  boost/AQ, AC-bias, RD/`--tx-bias`, sharpness, QM, in-loop-filter restraint, film-grain synthesis).
  <https://github.com/juliobbv-p/svt-av1-hdr/> and this repo's memory `svtav1-hdr-fork-analysis`.
- **svt-av1-hdr changed defaults (stills-relevant):** preset **4**, **10-bit**, variance boost
  **on** (strength 2), QM **on** (min-luma 6 / min-chroma 8), **sharpness 1**, `--sharp-tx 1`,
  `--tf-strength 1`, tune 3 (IQ) available. <https://github.com/juliobbv-p/svt-av1-hdr/>
- **The port's own 4-way pareto (repo memory `svtav1-pareto-4way-2026-07-16`)** measured, on
  single-frame all-intra p6, 8-bit, SSIMULACRA2:
  - **fork-tuning-on-v4.2 (mode1) beats mainline by ~8–12% bytes on photo** (best in the ssim2 50–60
    web band), and beats the shipped v4.1 fork everywhere.
  - **mainline wins on screen content** (v4.2's SC work); the shipped v4.1 fork is never the best.
  - `SVT_HDR_MODE=OFF` (mode0) is **byte-identical to mainline** — confirming the "gated delta on a
    mainline baseline" model.
  - **Caveat:** that advantage is measured at **8-bit / p6 / SSIM2 only**; the 10-bit fork advantage
    is *unmeasured*.

→ For **quality-photo AVIF**, the fork tuning is worth real bytes — so completing the port's
**HDR-fork mode** (mainline-off == mainline, on == rebased-fork) is genuinely valuable, but it is a
**gated delta that sits on top of mainline parity**, so mainline bd10 real-content comes first.

---

## 5. The real-world 80/20 config envelope for SVT-AV1 stills (ranked)

For the port's actual target (imageflow/zenavif AVIF stills via an SVT-AV1 backend), ranked by
real-world frequency × impact:

| Rank | Axis | The value that dominates real usage | Notes / evidence |
|---|---|---|---|
| 1 | **Bit depth** | **10-bit** (quality path) + **8-bit** (web-volume baseline) | 10-bit is the quality default even from 8-bit source; 8-bit is the sharp/libvips volume default. Both are first-class. |
| 2 | **Chroma** | **4:2:0** (the only functional SVT chroma) | 4:0:0 mono modest; 4:4:4/4:2:2 not functional in SVT → deprioritize. |
| 3 | **Preset** | **M2–M6** (quality) and **M8–M13** (fast/thumbnail) | M0 is a parity anchor, not a usage target. |
| 4 | **CRF/QP** | **~16–40** (quality band) with the **30–55** web-aggressive band equally weighted | do not over-weight HQ-only. |
| 5 | **Tune** | **1 (PSNR, baseline)** then **3 (IQ, stills)**; then 2/4 | tune changes bytes → parity axis. |
| 6 | **AVIF mode** | **`--avif 1`** on for real still pipelines | changes encode structure/RC → parity axis. |
| 7 | **PSY/fork tuning** | varboost-on, QM-on, sharpness 1, AC-bias (fork/quality path) | the ~8–12% photo win; gated `SVT_HDR_MODE` delta. |

**The canonical "quality SVT-AV1 AVIF still" invocation** (what the port should be byte-exact on):
```
SvtAv1EncApp -i in.y4m --avif 1 --tune 3 --crf 30 --preset 4 --input-depth 10 -b out.obu
```
(and its 8-bit sibling `--input-depth 8`, plus the fast-path `--preset 9`). Sources:
<https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/master/Docs/Parameters.md> ·
<https://github.com/strukturag/libheif/issues/1636> · <https://github.com/juliobbv-p/svt-av1-hdr/>

---

## 6. Reconciliation with the port's current state

From `STATUS.md` and the bd10/arbitrary-dims port-maps (2026-07-18/19):

- **bd8 4:2:0 — STRONG.** identity matrix 54/54; partial-SB + odd dims 101/101 across M6/7/8/9/10/13;
  mono conformance 1260/1260; chroma-420 conformance 1575/1575. This is the proven baseline.
- **bd10 4:2:0 — NARROW.** uniform 36/36 across M0/2/3/6/10/13; **non-flat bd10 only 2/2** (M10, M13)
  and *only* for the **DC-family / tx_depth-0 / rdoq-fp** subset. Out-of-envelope bd10 frames
  (directional or filter-intra intra, tx_depth>0, rdoq-0, non-uniform chroma) **fall back to the u8
  output** via the `bd10_tree_supported` gate. This is the crux gap.
- **HDR-fork mode — PARTIAL.** varboost math, chroma-q, light-RDOQ, noise-norm, AC-bias MD are wired;
  **per-SB delta-q wiring is the long pole and gates all HdrFork e2e** (fork defaults varboost on).
- **True-bd10-MD — IN FLIGHT** (the concurrent `claude-bd10-md` session).

**Where the port is mis-weighted vs real usage:** the *proof density* is on **bd8** while the
**quality-AVIF default is 10-bit**. The preset/dimension coverage is good; the bit-depth weighting is
the lever. The port's own P0 (10-bit + arbitrary dims, per `svtav1-production-priorities`) is exactly
right — this research **reinforces it and sharpens it to "bd10 *real-content* MD, not just flat."**

---

## 7. Recommended reprioritization of byte-identity effort (ordered)

Each item maps a real-usage config to a concrete port gap. Do them in this order for
soonest practical usefulness:

1. **Finish bd10 real-content mode decision (mainline, 4:2:0).** Close the `bd10_tree_supported`
   fallbacks so directional intra, filter-intra, `tx_depth>0`, rdoq-0, and the u16 chroma path are
   byte-exact — not just DC/flat. This is the `#94` follow-up list already in `STATUS.md`
   (`dr_predict_hbd`, `predict_filter_intra_hbd`, `quantize_b_hbd`, tx_depth>0 re-encode, u16 chroma,
   native u16 ingestion). **Why first:** 10-bit is the quality-AVIF default and today the port only
   handles flat 10-bit. Highest real-world leverage. *(Concurrent session already on this.)*
2. **Gate bd10 4:2:0 across the quality presets M2–M6 on real (photographic) content**, not only
   uniform/gradient — extend `real_image_matrix.sh` to bd10. Quality stills live at M2–M6; the port's
   bd10 matrix is uniform-only there. Add the **web-aggressive CRF band (30–55)** with the same
   density as the HQ band.
3. **Tune parity: tune 1 (baseline) then tune 3 (IQ, stills).** tune changes bytes; the stills tune
   (IQ) is the recommended AVIF tune and is very likely not yet byte-gated. Add a bd8+bd10 × {tune 1,
   tune 3} identity cell set.
4. **`--avif 1` still-mode parity.** Verify the still-picture RC / picture-set-count changes are
   matched at bd8 and bd10. Real AVIF pipelines (libheif) run with it on.
5. **Complete HDR-fork mode (`SVT_HDR_MODE`) — the per-SB delta-q / variance-boost long pole.** This
   unlocks the measured ~8–12% photo advantage and the quality-community defaults (varboost/QM/
   sharpness). It is a **gated delta on top of** items 1–4 (mainline parity is the floor), so it
   follows them. Re-measure the fork advantage at **10-bit** (currently only measured at 8-bit).
6. **Keep bd8 4:2:0 green as the anti-regression baseline** (still the web-volume leader) — no new
   drilling needed, just don't regress it.
7. **De-scope / re-annotate 4:4:4 & 4:2:2** in ACCEPTANCE-CRITERIA: mainline SVT-AV1 does not
   functionally support them. Keep **monochrome** (grayscale AVIF) at modest priority. Lossless (q0)
   stays low per existing priorities.

This ordering keeps the port's stated priority order (10-bit + arbitrary dims first, maintainability
continuous, lossless later, perf last) and simply re-weights the 10-bit work from "flat/uniform
proven" toward "real-content proven," where the quality-AVIF traffic actually is.

---

## 8. Caveats, unreachable sources, and open questions

- **Unreachable during research:** `wiki.x266.mov/*` refused connections (magic-DNS proxy at
  100.100.100.100); its **codecs.wiki mirror** served the same content and was used instead. Codec
  Wiki's SVT-AV1 deep-dive series is **v3.0.x-era (July 2025)** — its preset findings predate
  SVT-AV1 4.0's still-image work, so treat preset *specifics* as directional and prefer the mainline
  Parameters.md for current defaults/ranges.
- **The fork-tuning ~8–12% photo advantage is measured at 8-bit / p6 / SSIMULACRA2 only** (repo
  memory `svtav1-pareto-4way-2026-07-16`, self-flagged caveat). The 10-bit fork advantage is
  unmeasured — item 5 above should re-measure it.
- **"10-bit compresses SDR 5–12% smaller" is reported, not measured here.** The *directional* claim
  ("10-bit ≥ 8-bit efficiency for AVIF, even from 8-bit source") is well-supported by the AVIF
  quality guide; the exact percentage is third-party.
- **Volume vs quality split is real:** commodity web (sharp/libvips/Cloudinary) skews 8-bit today by
  tooling constraint; the codec-quality/archival path (avifenc, PSY forks) skews 10-bit. The port
  serves the latter, so 10-bit leads — but bd8 must never regress.
- **The port targets an SVT-AV1 backend specifically.** Much of the AVIF-quality literature
  recommends **aom** (for 4:4:4 + intra maturity). That is a statement about *aom vs SVT*, not about
  this port — svtav1-rs's niche is SVT-AV1's **4:2:0 stills** with tune-IQ/avif-mode + fork
  psychovisual tuning (where the pareto shows SVT-fork beating mainline on photo).

---

## Sources

- Codec Wiki — AVIF (bit depth, chroma, encoder choice, quality command): <https://codecs.wiki/docs/images/AVIF>
- Codec Wiki — SVT-AV1 encoder (chroma/bit-depth/tune): <https://codecs.wiki/docs/encoders/SVT-AV1>
- Codec Wiki — SVT-AV1 deep dive P1 (presets): <https://codecs.wiki/blog/svt-av1-fourth-deep-dive-p1>
- Codec Wiki — SVT-AV1 deep dive P2 (tune/varboost/params): <https://codecs.wiki/blog/svt-av1-fourth-deep-dive-p2>
- SVT-AV1 mainline Parameters.md (AUTHORITATIVE: color-format/depth/preset/tune/avif/crf defaults): <https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/master/Docs/Parameters.md>
- SVT-AV1 mainline user guide: <https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/master/Docs/svt-av1_encoder_user_guide.md>
- AOMedia — "SVT-AV1 4.0 Boasts Major API Updates" (tune IQ + MS-SSIM + AVIF mode + AC bias): <http://aomedia.org/blog%20posts/SVT-AV1-4_0-Boasts-Major-API-Updates/>
- Phoronix — SVT-AV1 4.0: <https://www.phoronix.com/news/SVT-AV1-4.0>
- svt-av1-hdr (juliobbv-p) — 10-bit/preset-4/varboost/QM/sharpness defaults: <https://github.com/juliobbv-p/svt-av1-hdr/>
- svt-av1-psy (archived) / svt-av1-psyex (BlueSwordM) — hbd-mds, tune, spy-rd, sharpness: <https://github.com/BlueSwordM/svt-av1-psyex>
- SVT-AV1-Essential (nekotrix) — forced/mandatory 10-bit: <https://github.com/nekotrix/SVT-AV1-Essential/discussions/20>
- libavif issue #2412 — PSY fork `--tune 4` (Still Picture) for AVIF: <https://github.com/AOMediaCodec/libavif/issues/2412>
- libheif issue #1636 — expose SVT `avif=1` still-mode: <https://github.com/strukturag/libheif/issues/1636>
- Halide — "Improving AVIF in Open Source" (tune-still-picture, ~15% AVIF gains, libaom tune-iq issues): <https://halide.cx/blog/improving-avif-in-open-source/>
- avifenc manual (depth/yuv/codec/speed defaults): <https://github.com/AOMediaCodec/libavif/blob/main/doc/avifenc.1.md>
- openaviffile — best AVIF settings (mainstream 8/10-bit + chroma + quality bands): <https://openaviffile.com/best-settings-for-avif-encoding/>
- sharp issue #4031 — higher-bit-depth AVIF (libvips 8-bit-by-default constraint): <https://github.com/lovell/sharp/issues/4031>
- Forasoft — 8-bit vs 10-bit (internal-precision efficiency argument): <https://www.forasoft.com/learn/video-encoding/articles/8bit-vs-10bit-encoding>
- JET encoding guide — SVT-AV1 params (preset 2/4, tune 1, crf 20–30): <https://jaded-encoding-thaumaturgy.github.io/JET-guide/master/encoding/svtav1/>
- OTTVerse — SVT-AV1 presets/CRF analysis: <https://ottverse.com/analysis-of-svt-av1-presets-and-crf-values/>

### Cross-referenced port docs / memory
- `docs/ACCEPTANCE-CRITERIA.md`, `STATUS.md`, `docs/bd10-port-map.md`, `docs/HDR-ON-4.2.md`
- memory: `svtav1-hdr-fork-analysis`, `svtav1-pareto-4way-2026-07-16`, `svtav1-production-priorities`, `svtav1-mainline-v420-final`

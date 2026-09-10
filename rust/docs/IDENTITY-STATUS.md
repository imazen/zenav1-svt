# Current identity status — 2026-09-08

Implementation snapshot: main `0cbd1279`. Historical campaigns, pins and old
first-difference investigations are [preserved verbatim](history/2026-09-08/rust/docs/IDENTITY-STATUS.md).
Use them as source-matched evidence, not as current open-bug lists.

Real-image byte identity, measured 2026-09-09 on current source: **180/180**
over 20 CID22-512 photos x presets {2,6,10} x cli_qp {20,40,55}
([record](../benchmarks/real_image_identity_2026-09-09.meta)). This is the
first such number since the "53 real-image parity cases fixed" claim, because
`tools/real_image_matrix.sh` could not build its oracle on any host until
`a046e68b`. Scope: 8-bit 4:2:0 512x512 stills on one x86-64 host.

The latest preceding eight-bit landing matrix was 1100/1100; the partial-chroma
and SB128 fixes also resolved all 53 historical witnesses in 168/168 replay
pairs. Neither result closes native10 or every optional/inter/HDR combination.
Pristine Mainline420 and Hybrid3115 are distinct C targets; source, HDR mode,
compiler and ISA belong in each parity record.

## Explicitly open native10 cells

376×512 photo fixture, `SVTAV1_BD=10 SVTAV1_HBD_SRC=1`:

| Native preset | QP | C bytes | Rust bytes | Status |
|---|---|---|---|---|
| 1 | 10 | 11465 | 11462 | open |
| 4 | 10 | 11752 | 11718 | open |
| 4 | 30 | 2827 | 2831 | open |
| 5 | 10 | 11913 | 11913 | open |

The stored C and Rust streams all decode independently. These are byte
mismatches, not demonstrated decoder failures. Hashes, exact fixture and
retained unproven experiment: [deferred manifest](deferred-native10-parity.json).

**These four cells do not share one root cause** (measured 2026-09-09): p1q10 and
p4q10 first diverge at arithmetic-coder op 0 in the `lr-taps` class, p4q30 at op
11758 and p5q10 at op 67328, both in unrelated CDF families, with p1q30 identical
across all 18413 ops as the control. A fix for one is not evidence for the others.
Start at [the coding-order witness](HANDOFF-2026-09-08-PARITY.md), not downstream
loop filters or the old raster-first pixel. **Updated 2026-09-09:** for p1q10 the
first real divergence is block **mi(32,36)** (pixel 144,128), a partition-size flip
(C bsize=3 vs port bsize=1) — not the previously recorded mi(48,8) CfL witness,
which is downstream of it. Established by capturing recon planes and the decision
tree from one run. Archive retrieval:
[native10 receipt](native10-handoff-receipt.json).

## Inter identity on REAL video (new surface, 2026-09-10)

Until now every inter cell in this repo encoded synthetic content, and the
multi-frame path built later frames by translating frame 0 by a global integer
offset — which open-loop ME finds exactly, so the residual SAD floors to zero
(`avg_me_sad=0`, `is_gm_on=0` across {gradient,diag,screen} × {64,128,256,512}).
The inter surface was being asserted against a motion field C's search never has
to work for.

`tools/real_video_inter_gate.sh` runs the two-frame differential on twelve I420
sequences cut from the six Xiph derf clips whose index entry reads *public
domain*, published at the R2 prefix `video/pd-derf-720p/` and fetched
anonymously. MEASURED 2026-09-10, 6 clips × {128×128, 256×256} × presets {6,8}
× cli_qp 40, 24 cells
([record](../benchmarks/real_video_inter_2026-09-10.meta)):

| frame | byte-identical |
|---|---|
| 0 (key) | **18 / 24** |
| 1 (inter) | **7 / 24** |

Two things this says that the synthetic grid could not:

- **Frame 0's six failures are all preset 6**; preset 8 key frames are 12/12.
  That is a *still* divergence reached through the video configuration, and it
  is a different surface from `real_image_matrix.sh`'s 180/180, which runs the
  all-intra path at 512×512 on CID22-512.
- **The control makes the gap explicit.** At the identical cell shape
  (128×128 q40 p6 frames=2) synthetic `gradient` is byte-identical on *both*
  frames, with frame 1 coding to 24 bytes — a skip. Real video at that shape
  diverges (`johnny` 33 B in C against the port's 36; `vidyo3` 113 against 101).

The pinned table in the gate is a measurement: a cell that regresses fails, and
a cell that improves fails too, as `PROMOTED`, so the table cannot silently go
stale.

**Vacuity note worth keeping.** All six clips are 30 fps material published at
60, so every frame is doubled. The first extraction scored `vidyo3` at mean
motion 20.4 while `|f1-f0|` was exactly 0.00, and both encoders coded that frame
to an identical 24 bytes — a cell that would have read as inter parity on real
video while asserting only that two encoders agree a repeated frame is a skip.
`tools/mk_video_assets.py` now decimates duplicates and scores motion as the
**minimum** over consecutive pairs, never the mean.

## imazen26 K300 production corpus (re-run after 48 days, 2026-09-10)

`tools/imazen26_gate.sh` asserts 40 cells over 20 images and covers content
classes no other corpus here reaches — bilevel patent scans, government document
pages, synthetic plots, AI clipart/illustration/product renders, manuscript
scans. **It had not run since the day it was written.** It ran once, 40/40, in
the commit that created it (`304c5832c`, 2026-07-24), against a *materialised*
cache at `/root/work/imazen26-cache/K300` on `dev-32gb` — a rented fleet box,
which the sweep's own meta records as having ended the run at its "box-lifetime
limit". That cache was never copied anywhere persistent (the box's migration
bundle, `~/work/hetzner-backup-dev-32gb` on lilith, deliberately excludes
re-downloadable corpora), so from that point `corpus_dir imazen26-cache/K300`
resolved to an absent path and every cell reported `MISSING`. It had never run
in CI.

**The corpus itself was never at risk, and that is the part worth knowing.**
K300's *selection* is git-tracked in imazen/codec-corpus at
`imazen-26/manifests/imazen26_representatives_K300_2026-06-14.tsv`: 300 rows of
`url  crop_label  content_class  cluster_id  cluster_size`, a k-means
representative pick over imazen-26's 2,160 images spanning all 20 content
classes, with every `url` pointing at the public
`codec-corpus.r2.imazen.org/imazen-26-png-v3/` prefix. All 20 of this gate's
images are in it. A derived cache whose recipe is versioned is not lost data —
only the materialisation was.

**Open discrepancy.** That manifest assigns a per-image crop region
(`crop_label`: `c50_bl`, `c50_center`, `c25_tl`, `full`, …). The gate
centre-crops every image via `crop:` at `IM26_DIM`. The 40 cells are
self-consistent and measured byte-identical that way, but they are not the
regions the representative selection chose.

Re-run 2026-09-10, first time in 48 days and first time ever in CI:
**40 / 40 byte-identical**, 61 s
([record](../benchmarks/imazen26_k300_2026-09-10.meta)). The gate was correct
all along — it had no input.

The assets are the 20 images centre-cropped to 512×512 (all the gate encodes,
since it feeds `crop:<png>` at `IM26_DIM=512`), published at the R2 prefix
`imazen26-k300-512/`: 290 MiB of originals down to 6.1 MiB. The substitution was
**measured**, not argued — the gate produced the same byte count for all 40
cells from both sets.

**Provenance gotcha worth keeping.** Images resolve from imazen-26's
`variant-sets/png-v3-index.tsv` by numeric `id`, never by filename: two of the
twenty carry transposed dimensions in the gate's own cell names (`3000x4000`
where the index says `4000x3000`), the defect `ACCESS.md` records for 196 of
2,160 images. Filename matching misses exactly those two.

## Separate evidence tracks

- [Named-reference audit](PARITY-REFERENCE-AUDIT-2026-09-08.md): pristine and hybrid
  source differences and scoped normal/research matrices.
- [Support audit](API-SUPPORT-AUDIT-2026-09-08.md): implementation reachability.
- [Refusal inventory](REFUSED-CONFIGS.md): generated source predicates, not a count
  of independent bugs and not proof that every refused configuration is invalid C.
- [C defects/oracle history](SUSPECTED-C-BUGS.md): preserve per-build/ISA distinctions.
- [Inter campaign](INTER-ENCODE-PLAN.md): historical experimental video evidence;
  the public streaming API is still unimplemented.

The latest full native workspace passed 2631/2631 at `0cbd1279`; that test
count is independent of the parity cell count. The earlier 106-case regression
number named a passing regression suite, not 106 newly confirmed flaws.

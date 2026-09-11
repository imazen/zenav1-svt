# Identity status — what is byte-identical, what is decoder-verified

This is the live parity document: read it for what the port matches today and
what it does not. It grows by section, each dated and each naming its
measurement — the heading date below is the section's, not the file's, and a
section without a date is older than the ones that have one.

**The two guarantees are not interchangeable.** Still images are byte-identical
to the C encoder. Inter/video is verified against a DECODER instead — the
encoder's own reconstruction must equal `aomdec`'s, frame for frame — because
that is the property a wrong stream actually violates and because the port's
inter search does not track C's bytes on all content. Never infer one from the
other.

## Still identity — 2026-09-08

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
  Drilled 2026-09-10 into a specific open bug
  ([record](../benchmarks/multiframe_keyframe_p2p7_2026-09-10.meta)): a key
  frame that is byte-identical to C **alone** stops being identical **when a
  second frame follows it**, at presets **2–7** only (p0, p8 and p10 are clean).
  The control is what makes it a finding — the same pixels at `frames=1` are
  byte-identical on all six cells, so it is not the content, the crop size or
  the still path. The harness is ruled out too: it hands the encoders different
  intra periods (C `-1`, port `64`), and matching them in either direction, or
  setting both to 255, leaves the bytes unchanged at 5053/5048. Minimal
  reproducer: `identity_diff_inter.sh 256 256 40 6 2
  rawseq:johnny_256x256_8f.i420`.
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

## Multi-frame video: SHIPPED in a measured envelope, gated against a decoder (2026-09-11)

**The envelope is 8-bit 4:2:0, presets 6..13.** Inside it,
`tools/video_selfcheck_gate.sh` is 144 of 144 cells (six public-domain derf
clips x qp {20,40,55} x presets 6..13) whose every frame of an 8-frame encode
reconstructs byte-identically to `aomdec`. Outside it an inter frame is
refused, not approximated, and each refusal carries its measurement:

| Refused | What was measured, 2026-09-11 |
|---|---|
| preset < 6 | preset 0 loses two of 18 cells, presets 1 and 2 lose two and one, preset 5 loses one. `SVTAV1_MFMV_OFF` returns every failing cell to 8/8, naming the temporal MV field — but signalling `use_ref_frame_mvs = 0` scores 151 of 163 against 156 with the field on, so the spatial-only stack has a second defect. Neither arm is shippable there |
| bit depth > 8 | three clips x qp {20,40} x presets {6,8,10} x 4 frames: 8 of 18 cells reconstruct as `aomdec` does, the rest drifting from frame 1, 2 or 3. `bd10_video_gate.sh` does NOT cover this — its decode leg asserts only that the stream parses |
| monochrome | no inter gate in this repo is monochrome, and a mono inter frame previously produced a stream both `aomdec` and `dav1d` rejected |

`SVTAV1_INTER_EXPERIMENTAL` lifts the preset and bit-depth floors for the
harnesses that must reach a low-preset tool. It is not a feature flag.

## How the envelope came to be (2026-09-11)

The heading below is the 2026-09-10 state and is kept because the trail is
useful; read this paragraph first. Both inter refusals are GONE, and so are the
`SVTAV1_INTER_EXPERIMENTAL` / `SVTAV1_INTER_CHAIN_EXPERIMENTAL` variables that
lifted them — `EncodePipeline`'s 4:2:0 entry points encode inter frames for
every caller. The standing guard is `tools/video_selfcheck_gate.sh`: the port's
own final reconstruction is byte-identical to `aomdec`'s on EVERY frame of an
8-frame encode, for all six public-domain derf clips at qp {20,40,55} — 18 of
18 cells. The monochrome arm still refuses inter, because no gate in this repo
covers it.

Byte-identity to C on the inter path is a separate, narrower claim and it is
NOT universal. MEASURED 2026-09-11 on the 96-cell frontier grid at frames=4:
95 cells identical on frame 0, 95 on frame 1, 60 on frame 2, 58 on frame 3.
The chain frames' gap concentrates in 72x72 (a PARTIAL superblock: 17 of its
24 cells differ at frame 2) and `gradient` content (19 of 24); `uniform` is
24 of 24 identical on every frame.

### The 2026-09-10 record

The port refused every frame past frame 1, so "does a longer encode decode?" was
unanswerable. `SVTAV1_INTER_CHAIN_EXPERIMENTAL` (default-off, measurement only)
lifted the second refusal — the byte-parity guard on an inter frame whose LIST-0
reference is itself inter — and made the answer measurable
([record](../benchmarks/video_multiframe_2026-09-10.meta)).

**It works.** On `vidyo3 256×256 q40 p6`, low-delay P: 2, 3, 4, 5 and 6 frames
decode completely under **both** `aomdec` and `dav1d`, and the two decoders'
output is byte-identical to each other. At 7+ frames aomdec reports *"Failed to
decode tile data"* on f6. C encodes and decodes 8/8 on the same `.yuv`, so the
defect is port-side. The limit is not fixed — `johnny` and `fourpeople` do 8/8,
`vidyo3` at q20 fails *earlier* (f5), and preset 8 is clean, the **same
preset 2–7 band** as the frame-0 divergence above.

**Root cause found and FIXED 2026-09-10**
([record](../benchmarks/deblock_skipinter_txsize_2026-09-10.meta)): a **skip
inter block's deblock transform size is the block's max, not its searched
`tx_depth`.** libaom's `get_transform_size` reads the per-TU var-tx size only
for `is_inter_block(mbmi) && !mbmi->skip_txfm`; a skip inter block falls through
to `mbmi->tx_size` = the block max, because a block that codes no residual never
puts its searched depth in the bitstream and a decoder cannot know it. The port
applied its `tx_depth` override regardless, so at the failing edges the decoder
read tx 16 and filtered 14-tap while the port read tx 8 and filtered 8-tap. An
intra block never takes that branch — which is precisely why the key frame was
always exact and only inter frames drifted.

| | f0 | f1 | f2 | f3 | f6 | decoded |
|---|---|---|---|---|---|---|
| before | identical | 33 px Δ1 | 86 px Δ6 | 851 px Δ34 | not produced | 6 of 8 |
| after | identical | **identical** | **identical** | 682 px Δ34 | 1020 px Δ36 | **8 of 8** |

The localisation is worth reusing: a qp sweep found that at qp 5–32 the signalled
`loop_filter_level[0]` is 0 and the recon is identical to the decoder's every
time, which isolated the deblock without needing any new instrumentation; the
diff footprint (six pixels each side of the edge) then named the filter length,
and `SVTAV1_PACKTREE` named the blocks as `inter=1 yeob=0 txd=1`.

**Two of the six clips now produce fully correct video** — `fourpeople` and
`kristenandsara` reconstruct byte-identically to the decoder for all 8 frames,
at both qp 20 and qp 40.

**A second defect remains, in the prediction path.** At qp 20 the signalled
`loop_filter_level[0]` is 0, so no filter stage runs on either side and any
mismatch is purely prediction + residual + reference. Four clips still drift
there, and **every one first drifts at f2 — the first frame whose reference is
itself an inter frame.** That is exactly the condition the original refusal
named, so its framing was right; what was missing was a way to measure it. f0
and f1 are correct on every clip at every qp tested.

**Narrowed 2026-09-10 to an exact signature**
([record](../benchmarks/chroma_subpel_inter_2026-09-10.meta)). At 4:2:0 a motion
vector is eighth-pel for luma and sixteenth-pel for chroma, so `mv.col` an **odd
multiple of 8** is integer in luma (`(mv*2) & 15 = 0`) and **half-pel in chroma**
(`mv & 15 = 8`). Classifying every inter block of one frame:

| luma integer | chroma sub-pel | blocks | contain a differing pixel |
|---|---|---|---|
| no | yes | 107 | 0 |
| yes | no | 16 | 0 |
| **yes** | **yes** | **3** | **2** |

107 ordinary sub-pel blocks and 16 fully-integer blocks are perfect; only that
class fails. Reproducer `vidyo3 256×256 p6 qp34 frames=2`, frame 1: the
reference is the byte-identical key frame, `loop_filter_level[0]` is 0 so no
filter stage runs on either side, luma is byte-identical, and only U and V
differ by ±1..2.

**Root cause found and FIXED.** The interpolation-filter search assigns its
winning pair to the candidate — so it is *signalled* — but rebuilds the
prediction only when `res.invalidates_luma_pred`. The search predicts **luma
only**, so chroma still holds the prediction made with the injector's filters
(packed 0, REGULAR both ways) whatever wins; `invalidates_luma_pred` answers a
different question, namely whether the *luma* buffer already holds the winner
because it was tried last. Instrumented proof on the failing block:
`best_filters=0x10001` (SMOOTH/SMOOTH), `was=0x0`, `invalidates_luma=false` —
nothing rebuilt, so the recon carried REGULAR chroma while the header said
SMOOTH. The fix rebuilds when `invalidates_luma_pred || (has_uv &&
best_filters != org)`; that is idempotent for luma, which already holds the
winning pair's prediction whenever the flag is false.

This is why it needed the odd-multiple-of-8 MV to show at all: an integer luma
MV applies no filter, so luma is right under either pair, while chroma's
non-zero phase exposes the wrong one.

**After the fix**, `johnny` joins the clean set: `johnny`, `fourpeople` and
`kristenandsara` all reconstruct byte-identically for all 8 frames, and `vidyo1`
moves from f1 to f4.

### What is left is the temporal motion-vector field, and only that

`SVTAV1_MFMV_OFF` (default-off, measurement only) builds the ref-MV stack from
spatial candidates alone and signals `use_ref_frame_mvs = 0` to match. With it,
**15 of 16 clip × qp cells reconstruct byte-identically to the decoder for every
frame** ([record](../benchmarks/video_mfmv_isolation_2026-09-10.meta)) — every
clip that drifted or stopped decoding becomes clean. So prediction, residual,
entropy, references, deblock and CDEF are all correct, and the temporal field
accounts for essentially the whole remaining defect.

The shape follows: `NEARESTMV`/`NEARMV` do not signal an MV, they **derive** it
from the ref-MV stack. Frame 1 is immune because its reference is the key frame
and C's projection returns nothing for it, which is why f0/f1 were always clean;
from the first frame whose reference is itself inter, a projection mismatch makes
encoder and decoder derive *different* MVs for the same block. That matches the
observed signature — large whole-block luma deltas spreading into neighbouring
intra blocks. It is what the original inter-chain refusal always named; what was
missing was a way to measure it.

Both halves of the flag must move together: gating only the header
desynchronises from frame 0, which produced 0-of-8 decoded streams that briefly
looked like evidence against the hypothesis and were evidence of nothing.

**One residual is not this defect:** `vidyo1` at qp20 still fails from f2 with
MFMV off — the only failing cell of sixteen, and now a clean reproducer.

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

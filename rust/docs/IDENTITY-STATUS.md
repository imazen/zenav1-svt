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

## Tune surface (`--tune`) — 2026-09-19

`SvtTune` (`zenav1-svt::SvtTune`, builder `with_tune`) exposes C's
`static_config.tune` values 0–4 as a typed enum: `Vq`, `Psnr` (default),
`Ssim`, `Iq`, `MsSsim`. Slot 5 is not exposed (mainline VMAF unmodeled;
the fork's FILM_GRAIN=6 is a different thing). The default is byte-neutral:
`Psnr` writes `tune = 1`, the value the pipeline already defaulted to.

Byte-parity vs C is per-variant, measured through `tools/issue9_knobs_gate.sh`
(not in CI; historically red — see below):

- `Vq` / `Psnr`: byte-identical on every probed cell (128²–512², p6/p10,
  qp 20/32/40/55, gradient + photo).
- `Ssim` / `Iq` / `MsSsim`: engage the per-16x16 SSIM-rdmult scaling
  (`pow`/`log`/`exp`, `port_md_lambda.rs` cross-ISA caveat) and diverge
  DECISIONALLY on part of the grid — frame headers match field-for-field
  and the variance-boost plan is C-exact, but near-tie decisions flip
  (first observed: a wiener `lr-taps` symbol; tuneiq-gradient-128-p6 q20:
  3199 B port vs 3205 B C). The gate reports 33/36 today; the three IQ
  cells have differed since the gate landed (`b80c2aa35`) and predate the
  AVX-512 campaign — documented, not silently green. Every variant's output
  is decoder-valid (aomdec-verified in the tune sweep).

`ZenEnhancement::StillImageTune` ("still-image-tune-v1") is the opt-in Zen
bundle: tune IQ's overrides verbatim, with caller-set values on
bundle-covered knobs surviving (its only delta vs `with_tune(Iq)` — with
no caller extras the stream is byte-identical to tune IQ). Requires
`EncodingPolicy::Zen` (SvtParity refuses all enhancements); validated for
all-intra 4:2:0, NOT byte-pinned to C — verified by aomdec decode and the
RD record in `benchmarks/still_image_tune_v1_2026-09-19.meta`. The
extras candidates measured on the subset (variance_octile 6–8, vb
strength/curve, ac_bias, min_qm, sharpness 3–6) were all neutral-or-worse
than plain IQ on ssim2-BD-rate — the best, sharpness 5, buys −0.34%
pooled median at p75 +0.55 (inside per-image noise) — so v1 pins nothing
beyond IQ's own bundle.

**Defect found and fixed in this work:** C's sb-size rule forces 64 when
`enable_variance_boost` is on (enc_handle.c:4077), applied AFTER the tune
overrides set it. The port derived `sb_size` in `new()` — before `hdr`
mutations were visible — so tune IQ / `with_variance_boost` / the still
recipe at sb128-deriving presets (-1..1 on large enough frames) emitted
an sb128 stream with per-64 delta-q symbols: "Failed to decode tile
data" under aomdec while C produced a valid sb64 stream. The derivation
now re-runs inside `encode_frame_impl` after the tune overrides
(matching C's `copy_api_from_app` → `set_param_based_on_input` order),
and the delta-q emission gate uses the real `sb_size` — the forced
`SVTAV1_SB=128`+VB combination (a port extension C cannot express) now
also produces a decodable stream. Verified: t3/t4/still at preset -1 all
decode; auto-derived ≡ explicit `SVTAV1_SB=64` byte-identical; VB-off
paths (t1/t2 at preset -1) byte-identical to pre-fix.

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

## Native10 cells — closed 2026-09-17

376×512 photo fixture, `SVTAV1_BD=10 SVTAV1_HBD_SRC=1`:

| Native preset | QP | C bytes | Rust bytes | Status |
|---|---|---|---|---|
| 1 | 10 | 11465 | 11465 | byte-identical |
| 4 | 10 | 11752 | 11752 | byte-identical |
| 4 | 30 | 2827 | 2827 | byte-identical |
| 5 | 10 | 11913 | 11913 | byte-identical |

All four formerly deferred cells now match C byte-for-byte and are permanent
`regression_spotcheck.sh` witnesses (`partial-chroma-native10-*`). Two root
causes, both u8 proxies where C reads real u16 data at `hbd_md`: the CfL
complexity detector used `(src8<<2)` vs u16 preds and raw u16 variance — C's
`vf_hbd_10` kernel pre-scales sum/sse to the 8-bit domain (~16× over-fire);
and the NSQ `skip_sub` quadrant gate scored u8 recon distances while C's
`calc_scr_to_recon_dist_per_quadrant` runs `svt_full_distortion_kernel16_bits`
on the u16 source vs u16 `cand_bf->recon`. Hashes, exact fixture and retained
unproven experiment: [deferred manifest](deferred-native10-parity.json).

Historical investigation trail (the divergences, now closed): p1q10 and p4q10
first diverged at arithmetic-coder op 0 in the `lr-taps` class, p4q30 at op
11758 and p5q10 at op 67328; for p1q10 the first real divergence was block
**mi(32,36)** (pixel 144,128), a partition-size flip. Established by capturing
recon planes and the decision tree from one run. Archive retrieval:
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
anonymously. MEASURED 2026-09-11, 6 clips × {128×128, 256×256} × presets {6,8}
× cli_qp 40, 24 cells:

| frame | byte-identical |
|---|---|
| 0 (key) | **22 / 24** |
| 1 (inter) | **10 / 24** |

Two findings moved since the 2026-09-10 measurement
([record](../benchmarks/real_video_inter_2026-09-10.meta), 18/24 and 7/24):

- **All six preset-6 frame-0 failures closed.** The "key frame diverges when
  a second frame follows it" bug
  ([record](../benchmarks/multiframe_keyframe_p2p7_2026-09-10.meta)) was the
  video arm's `skip_sub_depth_lvl` ladder: C's
  `svt_aom_sig_deriv_enc_dec_default` derives level 2 (`coeff_perc` 25) for
  enc_mode > M1 where the allintra ladder stays at level 1 (`coeff_perc` 15)
  through M7. The port had baked level 1 for every picture, so
  `eval_sub_depth_skip_cond1` never fired and flat ≤16×16 blocks were split
  that C keeps — measured at `johnny_256x256_8f` q40 p6, the 16×16 node at
  (16,32) has 39/256 = 15 % nonzero coefficients. `encdec_arm::apply` now
  stamps the per-arm level into `FunnelCfg::skip_sub_depth`, and the allintra
  bake is `<= M7 -> 1 else 2` rather than always 1.
- **Loop restoration is no longer gated on `is_key`.** C's
  `ppcs->enable_restoration` is picture-level (`wn > 0 || sg > 0`), and a
  flat GOP makes every frame `is_not_last_layer`, so C runs luma-only Wiener
  (`lr_type[0]=2`) on frame 1 where the port wrote `lr_type[0]=0`. The
  remaining frame-0 zeros are `vidyo3`/`vidyo4` 256×256 at **preset 8** — a
  different mechanism than the closed preset-6 one.
- **The control makes the gap explicit.** At the identical cell shape
  (128×128 q40 p6 frames=2) synthetic `gradient` is byte-identical on *both*
  frames, with frame 1 coding to 24 bytes — a skip. Real video at that shape
  closed for `johnny` after the 2026-09-13 inter-MD fix set (below);
  `vidyo3` still diverges (C 113 B against the port's 108).

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

## Multi-frame video: SHIPPED in a measured envelope, gated against a decoder (re-measured 2026-09-15)

**The envelope is 8-bit 4:2:0, presets -1..13.** Inside it,
`tools/video_selfcheck_gate.sh` is 270 of 270 cells (six public-domain derf
clips x qp {20,40,55} x presets -1..13) whose every frame of an 8-frame encode
reconstructs byte-identically to `aomdec`; presets -1..5 are also clean at
128x128 (126/126). The 2026-09-11 preset floor came off when the residual
low-preset drift was traced to the OBMC neighbour-prediction cache serving
one frame's predictions to the next (`NeighbourKey` carried no frame
identity) — `obmc_pred_arm::begin_leaf` now resets it per `evaluate_leaf`,
matching C's per-`md_encode_block` flag reset. The "spatial-only stack
defect" the 2026-09-11 measurement attributed to `SVTAV1_MFMV_OFF` was that
same cache; a 2026-09-15 MFMV_OFF re-sweep of presets -1..5 is 126/126 clean.

Outside the envelope an inter frame is refused, not approximated, and each
refusal carries its measurement:

| Refused | What was measured, 2026-09-11 |
|---|---|
| bit depth > 8 | three clips x qp {20,40} x presets {6,8,10} x 4 frames: 8 of 18 cells reconstruct as `aomdec` does, the rest drifting from frame 1, 2 or 3. `bd10_video_gate.sh` does NOT cover this — its decode leg asserts only that the stream parses |
| monochrome | no inter gate in this repo is monochrome, and a mono inter frame previously produced a stream both `aomdec` and `dav1d` rejected |

`SVTAV1_INTER_EXPERIMENTAL` lifts the bit-depth floor for the harnesses that
must measure a 10-bit inter frame. It is not a feature flag.

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
NOT universal. MEASURED 2026-09-13 on the 96-cell frontier grid at frames=4
(2026-09-11 was 95/95/60/58):
95 cells identical on frame 0, 95 on frame 1, 61 on frame 2, 59 on frame 3.
The chain frames' gap concentrates in 72x72 (a PARTIAL superblock: 17 of its
24 cells differ at frame 2) and `gradient` content (19 of 24); `uniform` is
24 of 24 identical on every frame.

The 2026-09-13 frame-1 gains came from four pieces of C behaviour the port was
missing, all in `generate_md_stage_0_cand_light_pd1` / `fast_loop_core`:

1. `merge_inter_cands` (mode_decision.c:3638-3643): when
   `min(md_me_dist, md_pme_dist) / (bw*bh) < (4*(63-qp))>>1`, EVERY inter
   candidate is CAND_CLASS_2 — one merged MDS0 pool, not per-mode lanes. The
   port now computes `md_me_dist`/`md_pme_dist` and stamps `cand_class`.
2. `ctx->global_mv_injection = ppcs->gm_ctrls.enabled` (enc_mode_config.c
   :7847/:7964): GLOBALMV injection is off wherever `gm_level` is 0 (p6+ on
   video); the port had hardcoded it on.
3. `mds0_use_hadamard_sb = false` on the video arm (:7916/:8032): MDS0's
   inter distortion is the VARIANCE arm (`fn_ptr->vf`), not hadamard SATD.
4. `dist_to_cost_th = 0` at mds0 level 2 (product_coding_loop.c:1309-1334):
   a candidate whose rateless distortion cost exceeds the running
   block-wide `mds0_best_cost` is dropped BEFORE its rate is priced. The
   port's intra lane had the gate; the inter lane now applies it too, with
   the best cost shared across classes exactly as C's per-block variable is.

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

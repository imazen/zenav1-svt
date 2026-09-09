# Native10 first-difference witness — current open cells, 2026-09-08

The validated eight-bit fixes are on main. The four native10 cells below remain open. The later unproven depth-refine edit is excluded from main's *tip* but not from main's history: it is commit `554412d9d`, an ancestor of main, reverted by `3522cd856`, so main's `depth_refine.rs` equals the pre-experiment file. Recover it with `git show 554412d9d -- rust/crates/svtav1-encoder/src/depth_refine.rs`. It is additionally byte-inert on p1q10 and structurally dead at preset >= 4, because its `bd10` arm (depth_refine.rs:1527) requires `!bypass_encdec` while leaf_funnel/rate_tables.rs:1254 sets `bypass_encdec = preset >= 4`. See ../../ENCODER-POLICY-GOAL.md for the full goal; latest implementation and validation are indexed in ../../CONTEXT-HANDOFF.md. The diagnostic measurements below retain their original source checkpoint.

## The four cells do not share one root cause (measured 2026-09-09, host i265)

Re-running the retained traces with `--verbose` recovers the first-divergence op index that the concise report suppresses (identity_diff.py:857 prints the `ALSO: tile-op` line only when the stage is not already `tile`, and the FRAME-OBU payload walk sets `stage="tile"` first for two of these cells):

```sh
cd rust
for c in bd10-p1-q10 bd10-p4-q10 bd10-p4-q30 bd10-p5-q10 bd10-p1-q30; do
  D=$HOME/tmp/svt-tracking/chroma-native-boundary/$c
  python3 tools/identity_diff.py --c-obu $D/c.obu --rust-obu $D/rs.obu \
    --c-trace $D/c.trace --rust-trace $D/rs.trace --verbose > ~/tmp/native10/$c-verbose.txt 2>&1
done
```

| cell | first divergent op | op-class |
|---|---|---|
| p1q10 | 0 | `lr-taps` (wiener_restore + literal run) |
| p4q10 | 0 | `lr-taps` (wiener_restore + literal run) |
| p4q30 | 11758 | cdf, `icdf0=16384`, unrecognized family |
| p5q10 | 67328 | cdf, `icdf0=20785`, unrecognized family |
| p1q30 (control) | none | traces identical for all 18413 ops incl. rng state |

The p1q30 control makes this anti-vacuous. **Op 0 is emitted before any SB3 block data**, so the `mi(48,8)` CfL witness recorded below cannot be the first divergence for p1q10 and p4q10. That discriminator has now been RUN for p1q10 (next section): the pre-filter recon planes DIFFER, so op 0 is a downstream symptom and the loop-restoration search is not the root cause. The same comparison has NOT yet been run for p4q10, the other op-0 cell — do that before assuming it behaves the same way.

p4q30 and p5q10 are separate investigations in unrelated families; identity_diff.py reports "unrecognized family" for both, so identifying which syntax element the `nsyms=14` CDF at p5q10 op 67328 belongs to is a source-reading sub-step. Do not assume a p1 fix moves them.

### Pre-filter reconstruction comparison, p1q10 (2026-09-09) — the LR-only reading is REFUTED

The op-0 `lr-taps` classification invited a tempting conclusion: that p1q10 is
purely a loop-restoration tap disagreement. It is not.

The retained pre-filter recon planes under
`~/tmp/svt-tracking/chroma-native-boundary/native10-prefilter/` (c.obu 11465 B,
rs.obu 11462 B, i.e. exactly this cell) were byte-compared, and independently
re-decoded from the raw u16-LE planes to avoid trusting the stored summary:

| plane | dims | differing samples | first difference |
|---|---|---|---|
| 0 (Y) | 376x512 | 88488 | (x=144, y=128), C=779 Rust=792 |
| 1 (U) | 188x256 | 17106 | (x=82, y=64), C=456 Rust=455 |
| 2 (V) | 188x256 | 16094 | (x=96, y=64), C=586 Rust=583 |

**Reconstruction differs before any loop filter runs.** So the block-level coding
decisions genuinely diverge, and the op-0 wiener_restore difference is a
DOWNSTREAM SYMPTOM — the restoration search is fed a different reconstruction and
therefore picks different taps, and those taps happen to be written early in the
bitstream. Do not chase the loop-restoration code for this cell.

**A NEW open question this raises.** The first differing luma sample (144,128) is
in SB4 under SB128 (origin 128,128; the docs' "SB3 origin(0,128)" only indexes as
3 with SB128, 3 SBs per row at width 376). But the recorded mode-decision witness
`mi(48,8)` = pixel (32,192) lies inside SB3, which is coded BEFORE SB4 — and no
luma sample inside SB3 differs at all. If the mi(48,8) UV/partition divergence
were the root cause, SB3's own reconstruction should differ. Either the
`native10-sb3/` drill was captured under a different configuration than this
prefilter run, or that candidate-cost divergence did not change SB3's final
reconstruction. Resolve that before spending more time on the CfL arbitration:
re-capture the SB3 drill and the prefilter planes from the SAME run.

### Decoded-pixel localization (2026-09-09, first run on any host)

`tools/decode_diff` had been unbuildable everywhere since its manifest hard-coded `/root/aom-rs`; repointed at the sibling zenav1-aom checkout it now runs. Decoding both stored streams of each cell gives:

| cell | first differing pixel | SB | differing luma px | share of 192512 |
|---|---|---|---|---|
| p1q10 | plane0 (64,0) c=132 r=133 | mi(0,16) | 89825 | 47% |
| p4q10 | plane1 (61,53) c=529 r=530 | mi(16,16) | 58139 | 30% |
| p4q30 | plane0 (51,248) c=247 r=246 | mi(48,0) | 39569 | 21% |
| p5q10 | plane1 (30,216) c=518 r=519 | mi(96,0) | 9684 | 5% |
| p1q30 (control) | — | — | 0 | IDENTICAL decoded output |

This **corroborates the op-class split and further undercuts the single-CfL story**. The two op-0 `lr-taps` cells have by far the largest pixel divergence and p1q10's first differing pixel is at (64,0) — the first superblock row, *earlier in raster order* than the SB3 origin(0,128) witness below. A loop-restoration tap difference is applied frame-wide, so both the early first-pixel and the ~47%/30% spread are what an LR-class divergence looks like, and the "first differing pixel" is a weak localizer for those two cells: read the NDIFF magnitude instead. p5q10's 5% is the genuinely localized one.

Reproduce (instant, no encode — the stored OBUs are enough):

```sh
DD=rust/tools/decode_diff/target/release/decode-diff
for c in bd10-p1-q10 bd10-p4-q10 bd10-p4-q30 bd10-p5-q10 bd10-p1-q30; do
  P=$HOME/tmp/svt-tracking/chroma-native-boundary/$c
  echo "== $c"; "$DD" "$P/c.obu" "$P/rs.obu"
done
```

Two eight-bit defects fixed in pipeline.rs: partial-edge chroma source reads now use SB-wide edge-replicated source/MD canvases; whole-SB128 depth-limit folding now includes partial 64x64 quadrants. All 53 historical mismatches on the 376x512 photo are fixed, with 168/168 fresh C/Rust byte-identical replays. Fixture and three regression witnesses are committed under rust/tools/fixtures and regression_spotcheck.sh.

Before the subsequent native10 source/debug edits, local gates passed: 2627/2627 workspace nextest, 139/139 spotchecks, 1100/1100 matrix. Evidence lives under ~/tmp/svt-tracking/{chroma-stride-nextest.log,chroma-stride-final-spotcheck.log,chroma-stride-full1100.log,chroma-parity-replay/summary.json}. These historical counts describe that checkpoint. Current main separately passed 2631/2631 workspace tests; the four byte divergences are still not passing cells.

Expanded real-photo matrix is 16/20: eight-bit10/10, genuine native10 (SVTAV1_HBD_SRC=1) 6/10. Failures p1q10, p4q10, p4q30, p5q10. p-1 and p0 pass both QPs/depths. This is native10 SDR-derived input, not an HDR corpus claim.

Native p1q10 remains C11465B/Rust11462B after depth_refine.rs quad_rec_dists was corrected to read actual fx.src10 rather than widened u8. That change is byte-inert here and was reverted; see the depth-refine note in the header above. It is not WIP and not a pending fix.

First divergence in BLOCK coding order (not entropy-op order — see the op table above, where p1q10/p4q10 diverge at op 0): SB3 origin(0,128), block mi(48,8), first luma pixel(33,192). The previously noted raster-first mi(32,36) is a downstream SB4 difference. Seed CDFs agree through SB3; remove C's initial extra seed dump before comparing. First real seed drift is SB4.

At mi(48,8), square32 (146471172) and square16 (34068505) costs match exactly. Four children then differ, not one — re-read from c.pickpart:389-393 and rs.log on 2026-09-09:

| child | C uv / cost | Rust uv / cost |
|---|---|---|
| HORZ nsi=0 | 2 / 13362812 | **13 (CfL)** / 12799315 |
| HORZ nsi=1 | 3 / 19008564 | 11 / 18589550 |
| VERT nsi=0 | **13 (CfL)** / 15223893 | 0 / 15512721 |
| VERT nsi=1 | 7 / 17533764 | 9 / 17631632 |

HORZ4's four child costs and uv modes all match C exactly (3162666/10412626/8337602/10114308, sum 32027202), which is why C's PARTITION_HORZ_4 beats Rust's cheaper HORZ sum 31559790.

Two consequences the earlier note missed. **CfL flips in both directions** — Rust chooses CfL where C does not at HORZ nsi=0, and C chooses CfL where Rust does not at VERT nsi=0 — so this is a near-tie precision problem, not a one-sided gate-polarity bug. And **the independent-UV search is not at fault**: rs.log's `NSQDBG UVTAB mi=(48,8) 16x8` index 7 is `(2,0)` = UV_H_PRED, exactly C's uv=2. Only the CfL-versus-independent arbitration diverges (leaf_funnel/mds3.rs, C `check_best_indepedant_cfl`), not the table feeding it.

The structural suspect is precision, not logic: `try_encode_frame_420_hbd` (pipeline.rs:1710-1735) stores the real u16 planes but drives the core with MSB-truncated `>> 2` u8 planes, and any bd10 stage not yet threaded re-widens `<< 2`. Three such sites sit on the CfL decision path. Investigate there, not downstream loop-filter levels. Prove against actual C before changing candidate decisions.

Local diagnostics: ~/tmp/svt-tracking/chroma-native-boundary/native10-sb3/{c.pickpart,rs.log}, native10-prefilter, native10-seeds and summary.json. New env-off diagnostic hooks: SVTAV1_RECON10_BIN (Rust native prefilter planes), SVT_FINAL_MI_OUT (C completed MI grid), SVT_PD0_ALL_OUT (C all PD0 candidate costs). All local heavy jobs were terminal at handoff.

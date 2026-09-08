# Native10 first-difference witness — current open cells, 2026-09-08

The validated eight-bit fixes are on main. The four native10 cells below remain open. The later unproven depth-refine edit is excluded from main and retained in the handoff archive. See ../../ENCODER-POLICY-GOAL.md for the full goal; latest implementation and validation are indexed in ../../CONTEXT-HANDOFF.md. The diagnostic measurements below retain their original source checkpoint.

Two eight-bit defects fixed in pipeline.rs: partial-edge chroma source reads now use SB-wide edge-replicated source/MD canvases; whole-SB128 depth-limit folding now includes partial 64x64 quadrants. All 53 historical mismatches on the 376x512 photo are fixed, with 168/168 fresh C/Rust byte-identical replays. Fixture and three regression witnesses are committed under rust/tools/fixtures and regression_spotcheck.sh.

Before the subsequent native10 source/debug edits, local gates passed: 2627/2627 workspace nextest, 139/139 spotchecks, 1100/1100 matrix. Evidence lives under ~/tmp/svt-tracking/{chroma-stride-nextest.log,chroma-stride-final-spotcheck.log,chroma-stride-full1100.log,chroma-parity-replay/summary.json}. These historical counts describe that checkpoint. Current main separately passed 2631/2631 workspace tests; the four byte divergences are still not passing cells.

Expanded real-photo matrix is 16/20: eight-bit10/10, genuine native10 (SVTAV1_HBD_SRC=1) 6/10. Failures p1q10, p4q10, p4q30, p5q10. p-1 and p0 pass both QPs/depths. This is native10 SDR-derived input, not an HDR corpus claim.

Native p1q10 remains C11465B/Rust11462B after depth_refine.rs quad_rec_dists was corrected to read actual fx.src10 rather than widened u8. That change has no demonstrated fixing witness yet and remains WIP.

Correct first coding-order divergence: SB3 origin(0,128), block mi(48,8), first luma pixel(33,192). The previously noted raster-first mi(32,36) is a downstream SB4 difference. Seed CDFs agree through SB3; remove C's initial extra seed dump before comparing. First real seed drift is SB4.

At mi(48,8), square32 and square16 costs match exactly. First HORZ16x8 child differs in UV choice: C mode7 UV2 txdepth2 rate68246 dist32752 cost13362812; Rust mode7 UV13 CfL txdepth2 rate64525 dist32256 cost12799315. HORZ4's four child costs all match C; Rust's cheaper HORZ wins instead. Investigate native CfL versus independent-UV arbitration/detector precision in leaf_funnel/mds3.rs, not downstream loop-filter levels. Prove against actual C before changing candidate decisions.

Local diagnostics: ~/tmp/svt-tracking/chroma-native-boundary/native10-sb3/{c.pickpart,rs.log}, native10-prefilter, native10-seeds and summary.json. New env-off diagnostic hooks: SVTAV1_RECON10_BIN (Rust native prefilter planes), SVT_FINAL_MI_OUT (C completed MI grid), SVT_PD0_ALL_OUT (C all PD0 candidate costs). All local heavy jobs were terminal at handoff.

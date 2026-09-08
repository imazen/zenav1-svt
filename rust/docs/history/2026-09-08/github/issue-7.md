# Historical GitHub issue #7: Tracking: capability gaps vs C SVT-AV1 v4.2.0 (still-image encoder roadmap)

Snapshot before the 2026-09-08 handoff cleanup. State at capture: **OPEN**.
Original: https://github.com/imazen/zenav1-svt/issues/7. Current disposition: [issue audit](../../../OPEN-ISSUES-AUDIT-2026-09-08.md).

## Original body

Canonical tracker for the capability distance between this **still-image (AVIF/all-intra)** port and C SVT-AV1 v4.2.0. The port is byte-identical to C on a narrow envelope; this issue records what is FULL / PARTIAL / ABSENT and links the sub-issues. Source-verified 2026-07-24.

### Key framing: C's *shipping* envelope is narrow, and the port nearly fills it
C v4.2.0's `verify_settings` (`enc_settings.c`) only accepts **8/10-bit** (`:460` rejects 12-bit) and **4:2:0** (`:470` `"Only support 420 now"` rejects 422/444/400). The 422/444/12-bit machinery exists in the tree but is dead-gated. So **format-wise the port already matches C's shipping envelope** — the real distance is *feature*-level (rate control, inter, superres, segmentation, HDR metadata), NOT chroma/bit-depth.

### Two corrections vs an earlier draft (recorded so they don't recur)
- **Rate control is NOT a still-image gap (EMPIRICALLY VERIFIED 2026-07-24).** SVT-AV1's default/recommended still mode is CRF, but for a single frame `--qp N` == `--cqp N` == `--crf N` byte-for-byte (aq-mode-2's deltaq needs TPL lookahead, which one still frame has none of — `r0` inits 0, pcs.c:1299). Proof: `rust/benchmarks/crf_cqp_equivalence_2026-07-24.md`. The port's `qp=N` already emits the default-CRF bytes; `RcMode::Crf`==`Cqp` is correct-by-design. *(This corrects an earlier draft of this very issue that called CQP-only the #1 still gap.)*
- **422/444 and 12-bit are NOT port gaps.** C rejects them at init (`enc_settings.c:470` non-420; `:460` non-8/10-bit). Both sides reject them, so the port is faithful — they'd become gaps only if C is patched to enable them (to provide an oracle). Earlier drafts wrongly credited C with shipping 422/444/12-bit.

### Gap table (verdict = parity in the port's tested envelope)
| Dimension | C v4.2.0 (shipping) | Port | Verdict |
|---|---|---|---|
| Frame types / inter | KEY + INTER P/B + S-frame, ME, DPB | KEY still only (inter = dormant homegrown scaffolding) | **ABSENT** |
| **Rate control** | CQP/CRF/VBR/CBR, 2-pass, aq, TPL | **CRF ≡ CQP ≡ QP for one still** (aq-mode-2 deltaq is TPL-gated, inert for a single frame — *verified* vs C, `--qp N`==`--cqp N`==`--crf N`). VBR/CBR = multi-frame/degenerate for one still | **Still RC: FULL** — port's `qp=N` already emits the default-CRF bytes. VBR/CBR size-targeting only meaningful across frames |
| Bit depth | **8/10 only** (12-bit rejected, `:460`) | 8 byte-exact; 10 via `u8<<2` widening (no native u16 input) | 8 FULL · **native-10-bit-input PARTIAL (#6)** · 12-bit N/A (both reject) |
| Chroma | **420 only** (422/444 dead-gated `:470`) | 420 + mono | **FULL parity** (both reject 422/444) |
| Dimensions | arbitrary | partial-SB byte-match; non-64 at preset ≥ 6 | FULL |
| Presets 0–13 | −3..13 | 0–10 byte-exact; 11–13 clamp to M9 | FULL |
| Intra tools | DC/dir+angle/SMOOTH/PAETH/filter-intra/CfL/palette/IntraBC | all present & byte-verified | FULL |
| Inter tools | compound/OBMC/warp/interintra | none | ABSENT |
| Post-filters | DLF/CDEF/Wiener+SGR/superres/film-grain | DLF+CDEF+Wiener byte-exact (SGR not searched — C skips it at this ENC_MR too, not a divergence); superres = stub; FG = fork synthesis | DLF/CDEF/Wiener FULL · superres ABSENT · FG PARTIAL |
| Tiling | rows/cols | byte-identical; deterministic tile-parallel | FULL |
| Screen content | palette/IntraBC/detect | palette FULL, IntraBC partial, detector ported | PARTIAL |
| Segmentation / delta-q | aq, segments, ROI, TPL | delta-q via fork var-boost only; no segments/ROI/TPL | delta-q PARTIAL, rest ABSENT |
| Temporal filter / ALT-REF / GOP | tf, hierarchical GOP | dormant (verified path intra_period ≤ 1) | ABSENT |
| Tune modes | VQ/PSNR/SSIM/IQ/… | 6 fork tunes, decode-gated, not byte-gated | PARTIAL |
| Color / CICP | CICP + mastering-display + content-light | CICP byte-matches defaults; HDR static-metadata OBUs not emitted | CICP FULL · HDR metadata ABSENT |
| Output | OBU/IVF, sequences | one still OBU (AVIF mux = zenavif) | still FULL · animation ABSENT |

Survey corrections already folded in: port is **tile-parallel** (not single-threaded, `thread::scope` `pipeline.rs:6854`), it **is** SIMD (archmage), SGR is in-envelope-faithful, and `COVERAGE.md`'s "0/121 fields" is an API-shape scoreboard, not a feature measure.

### Biggest gaps, ranked by still-image relevance
1. **Native 10-bit input** — #6 (u16 entry point; 10-bit today is `u8<<2`-synthesized). *(Rate control is NOT here — CRF≡CQP for stills, verified; the port already matches the default.)*
3. **Superres · HDR static metadata** — superres stub-only; mastering-display/content-light OBUs unemitted despite HDR framing.
4. **Segmentation / ROI / TPL-delta-q** — only fork var-boost delta-q exists.
5. **Inter / GOP / ALT-REF** — the whole video half; out of scope by design.

### Sub-issues
- #5 — lossless (QP 0) emits garbage pixels
- #6 — public u16 10-bit-source entry point
- 422/444 + 12-bit intentionally NOT tracked — dead-gated in C; no oracle without patching C first.

Open verification note: C's mono path — `verify_settings:470` rejects `EB_YUV400` yet the CLI maps `mono→EB_YUV400` (`:1786`); the port's mono byte-parity target (420-with-mono-signal vs a 400 path) is worth confirming, tracked here.

Not a work commitment — a map. Still-image parity items (RC, native 10-bit input, superres, HDR metadata) are the actionable near-term set.


## Historical comment — 2026-07-24T10:31:05Z

**Mono verification (the open note) — RESOLVED.** C SVT-AV1 v4.2.0 cannot encode monochrome: `verify_settings` rejects `EB_YUV400` (`enc_settings.c:470`) and the still/avif capture harness is hardwired to `EB_YUV420` (`capture_c_trace.c:164`, `avif=true`). So **there is no C mono oracle**, and the port's monochrome (`encode_frame` → `mono_chrome=1`) is validated by **decode-conformance** — `recon_parity` (encoder-recon == aomdec, bit-exact) + `decode_conformance` (aomdec + dav1d) — not byte-identity to C. The byte-vs-C gates (`identity_matrix`, bd10, …) run the 4:2:0 path. Recorded as confusion-guard #6 in rust/CLAUDE.md. No mono byte-gap exists to close (would require patching C to un-gate `EB_YUV400` first).

## Historical comment — 2026-07-24T10:40:57Z

**Rate control — RESOLVED, and it is NOT a still-image gap (correcting my own earlier framing in this issue).** Empirically verified with the built C encoder (SvtAv1EncApp v4.2.0): for a single still frame, `--qp N` == `--cqp N` == `--crf N` **byte-for-byte** across preset {0,8} × qp {20,40,55}. Root cause: aq-mode-2's deltaq (`svt_aom_sb_qp_derivation_tpl_la`, rc_aq.c:899) only fires under `tpl_ctrls.enable && r0 != 0` — TPL lookahead, which one still frame has none of (`r0` inits 0, pcs.c:1299). SVT-AV1's default/guide-recommended still mode is CRF, so the port's `qp = N` (CQP) already emits the default-CRF bytes; `RcMode::Crf` == `Cqp` is correct-by-design, not a stub. Evidence: `rust/benchmarks/crf_cqp_equivalence_2026-07-24.md` (landed on main @ 0b0627197). VBR/CBR bitrate-targeting remains multi-frame/degenerate for a single AVIF (you iterate CRF to hit a size).

## Historical comment — 2026-08-04T11:53:15Z

**Roadmap update against `57670b3ab`** — one tracked gap closed, one narrowed, plus a new cross-cutting caveat that belongs in this tracker.

**Arbitrary dimensions / partial superblocks — CLOSED for 8-bit.** Was 7-12 of 36 partial-SB cells at presets 0-4 and 25/36 at p5. Now **36/36 at p0/p1/p2/p3/p5 and 34/36 at p4**, with the 64-aligned columns unchanged at every preset. Two roots, both in the PD1 depth-refinement path:

1. `pipeline.rs` required a COMPLETE superblock before running the C-faithful PD1 refinement, so a partial SB silently took the plain PD0 fixed tree — a search C never runs. C runs PD0 + PD1 at every superblock; `set_blocks_to_test` only narrows which d1 shapes a boundary node may test.
2. A boundary PD0 leaf must NEVER be refined: C's `tested_blk[PART_N][0]` is false at a single-edge node (`product_coding_loop.c:10548-10560`), so every PD1 gate reading a PD0 cost is skipped there (`enc_dec_process.c:1550/1566/1586/1634/1698-1712/1859/1868`). The port had one `tested` flag and fed the boundary RECT cost to those gates as a square PART_N cost.

Gates: default 8-bit **1098/1098 + 2 pinned** (was 1036/1036 + 4), `partial_sb_gate` **146/146** (was 104), dims tier default widened to presets 5..13. Non-multiple-of-64 verified end-to-end at preset >= 6 up to 1000x700 — encode, aomdec decode, and byte-parity with C.

**bd10 still requires 64-aligned dims** (`pipeline.rs:825`, typed refusal) — no bd10 producer is partial-SB aware. That is now the narrower form of this gap.

**New cross-cutting caveat for the tracker: byte-parity is a PER-ISA claim.** C's own encoder emits different bytes on x86-64 vs aarch64 for some inputs. Three instances known, and they do not share a mechanism — the first two are bd10/preset 7/screen (explained by the `svt_aom_hadamard_32x32_c` vs `_avx2` disagreement) but the third is **bd8/preset 0/gradient/partial-SB**, which that explanation does not cover. The port is NOT the variable side: `tier_invariance.rs` encodes the same cells under every SIMD dispatch tier and is green, and the scalar tier is portable integer Rust.

Bounded rather than open-ended: the x86-64 CI run that caught the third cell passed the full 1098-cell gate, identical to what passes locally on aarch64 — so ~1240 cells run on both architectures with exactly one disagreement. Rare, but not predictable, so a new cell stays provisional until seen green on both. Details in `docs/SUSPECTED-C-BUGS.md` #9. CI is x86-64 only today; a second aarch64 runner would find these as a set instead of one at a time.


## Historical comment — 2026-08-29T00:36:14Z

## Gap table re-verified against `main` @ `7fc16c51`, then re-prioritised by what actually blocks zenavif

Three parts: (i) the table, each row checked against current source rather than carried forward; (ii) a priority order justified by estimated real-world still-AVIF usage **and** by what blocks zenavif from making `SvtRs` its default backend; (iii) native 10-bit input — status, a new gate, and a measurement that changes what "byte-identical to C" means at bd10.

### (i) Re-verified gap table

Changed rows are marked. Everything else re-read and unchanged.

| Dimension | C v4.2.0 (shipping) | Port, today | Verdict |
|---|---|---|---|
| Frame types / inter | KEY + INTER + S-frame | KEY still only | ABSENT (by design) |
| Rate control | CQP/CRF/VBR/CBR | CRF ≡ CQP for one still (verified), **now incl. FRACTIONAL CRF** — `RcConfig::crf(f32)`, `extended_crf_qindex_offset` consumed at rc_crf_cqp.c:471, 19/19 byte-identical incl. the qp-63 extended range | **CHANGED → still RC FULL** |
| Bit depth | 8/10 only (12 rejected `:460`) | 8 byte-exact; **10-bit NATIVE u16 input landed** (chunks 1+2, `try_encode_frame{,_420}_hbd`) — no `u8<<2` anywhere on that path | **CHANGED → 8 FULL · 10 FULL within 64-aligned dims · 12-bit N/A** |
| Chroma | 420 only (422/444 dead-gated `:470`) | 420 + mono | FULL |
| Dimensions | arbitrary | 8-bit partial-SB byte-identical (`partial_sb_gate` 146/146); **bd10 still needs 64-aligned dims**; monochrome needs 8-aligned and preset ≥ 6 | PARTIAL (bd10 + mono) |
| Presets | −3..13 | 0–10 byte-exact; 11–13 clamp to M9 as C does | FULL |
| Intra tools | full set | all present, byte-verified | FULL |
| Post-filters | DLF/CDEF/Wiener+SGR/**superres**/FG | DLF+CDEF+Wiener byte-exact; **superres implemented and byte-gated** (`tools/superres_gate.sh`: parity + decodability + anti-vacuity, preset 8 × denom 9..16); SGR MR-only in C so unreachable here | **CHANGED → superres was "stub", now gated** |
| Tiling | rows/cols | byte-identical, deterministic tile-parallel | FULL |
| Screen content | palette/IntraBC/detect | palette FULL, IntraBC partial | PARTIAL |
| Segmentation / delta-q | aq, segments, ROI, TPL | variance-boost delta-q (mainline + fork), recon-parity gated 60/60; no segments/ROI/TPL | PARTIAL |
| Tune modes | VQ/PSNR/SSIM/IQ/… | **tune IQ is now BYTE-gated against C, not just decode-gated** — and it is NOT byte-identical: 1/6 cells match, residual 1–6 bytes | **CHANGED → was "not byte-gated"; now measured** |
| Colour / CICP | CICP + mdcv + clli | CICP byte-matches; **`chroma_sample_position` now exposed** (2/2 byte-identical); HDR static-metadata OBUs still unemitted | **CHANGED (csp)** · HDR metadata ABSENT |
| Quality knobs | `--max-tx-size`, QM, variance boost, sharpness | all reachable in mainline; **`max_tx_size` now byte-gated 9/9** | **CHANGED** |
| Lossless (QP 0) | supported | **implemented** (issue #5), byte-identical to C at presets 4..13 | **CHANGED — sub-issue #5 closed** |
| Output | OBU/IVF, sequences | one still OBU | still FULL · animation ABSENT |

Two sub-issues closed since this tracker was written: **#5** (lossless) and **#6** (u16 entry point). **#3** (crate consolidation) closed today.

### (ii) Priority

**Usage estimate, and its basis.** I have no measured distribution of AVIF stills on the web, so this is an ESTIMATE and I am naming what it rests on rather than dressing it as data: the AVIF/HEIF still profile, what browsers accept, what phone camera pipelines emit, and what the CDN-side encoders in this workspace are pointed at.

- **8-bit 4:2:0 photographic — dominant.** Effectively all CDN/CMS-transcoded AVIF. This is the case the port is byte-identical on.
- **10-bit — a real and growing minority**, and the one segment where the *format* is the reason to use AVIF at all. Two sources: phone HDR captures (PQ/HLG BT.2020) and 8-bit masters encoded at 10 bits because AV1 10-bit is measurably better even for SDR. Both are 4:2:0.
- **Alpha — common** (logos, product cut-outs, UI assets). Encoded as a separate monochrome auxiliary item, so mono is not a niche path for a still encoder; it is the alpha path.
- **Screen content / line art — a modest but non-trivial share** (screenshots, diagrams, UI). Disproportionately sensitive to palette + IntraBC.
- **4:4:4 — near zero in delivered AVIF** and, decisively, *not a port gap*: C rejects it at `enc_settings.c:470`.
- **12-bit — essentially nil**, and C rejects it at `enc_settings.c:460`. **N/A unless the owner un-gates C** to provide an oracle; until then there is nothing to be byte-identical to.
- **Animation — out of scope** for a still-image port.

**The stronger ordering criterion: what blocks zenavif from defaulting to `SvtRs`.** From `zenavif/CLAUDE.md` (`encode-svt-rs`) and `src/encoder_svt_rs.rs`, the seam's envelope today is 8-bit + 10-bit stills, 4:2:0 + mono, with these gates:

1. **`svt_rs_dims_error`: 64-multiple dims at every speed; arbitrary only at speed ≥ 5 (SVT preset ≥ 6)**, because presets 0–5 were not partial-SB-C-identical. **That premise is now stale** — `partial_sb_gate` is 146/146 including presets 0–5 (closed 2026-08-04). The seam can lower its gate; nothing upstream blocks it.
2. **`svt_rs_depth_error`: 10-bit monochrome only at SVT preset ≥ 9 (speed ≥ 7)**, and since an AVIF alpha item must match the colour item's depth, **every 10-bit RGBA encode is forced to speed ≥ 7**. This is the single sharpest upstream blocker: it makes 10-bit-with-alpha unavailable at the quality end of the speed ladder.
3. **bd10 requires 64-aligned dims** (`pipeline.rs` typed refusal) — so 10-bit + arbitrary dimensions is unreachable at any speed.
4. **RD at the aggressive end.** `backend_sweep_2026-07-22`: SvtRs is 0.93× bytes at matched ssim2 at speed 2, but **1.02–1.07× at speed 6 and worse below q30**. Web delivery lives at aggressive quality, so "worse below q30" is the commercially load-bearing number, not the s2 win.
5. **Speed-ladder misalignment** (linear speed→preset on both sides, deliberately not re-fit — owner decision). It distorts every backend comparison.
6. The seam's own doc is stale on one point worth correcting when someone next touches zenavif: it says the 10-bit post-filter searches "still decide on MSB-truncated planes (upstream hbd chunk 2)". **Chunk 2 landed** — deblock, CDEF and Wiener all read the native u16 source now.

**Proposed order.** Ranked by (blocks-zenavif-default) × (estimated usage), not by how interesting the work is:

1. **10-bit monochrome below preset 9** — unblocks 10-bit + alpha across the whole speed ladder. Highest ratio of "unblocks a common case" to work: the missing piece is a bd10 level producer for mono at presets < 9.
2. **bd10 at non-64-aligned dimensions** — unblocks 10-bit for arbitrary sizes; today a 10-bit 1000×700 image is simply refused. Needs a partial-SB-aware bd10 producer.
3. **RD at low quality (q ≤ 30)** — the byte-count regression versus the other backend at exactly the settings web delivery uses. Not a conformance gap, a competitiveness one; it is what a "make SvtRs default" decision will actually turn on.
4. **IntraBC / screen content completion** — the modest-share content class with the largest per-image gap.
5. **tune IQ byte-identity** — the residual measured today (below). Small, and tune IQ is the still-image tune SVT-AV1 itself recommends.
6. **HDR static metadata OBUs** — LOW for AVIF specifically: zenavif writes `clli`/`mdcv` container-side, which is where the AVIF spec puts them. Only matters for raw-OBU consumers.
7. **Segmentation / ROI / TPL delta-q** — no still-image use case is blocked by their absence.
8. **12-bit, 4:4:4, animation** — N/A. C rejects the first two; the third is out of scope.

### (iii) Native 10-bit input — landed, plus a gate that goes further, plus a measurement that reframes bd10 parity

**Status: the real u16 entry IS the one zenavif uses.** `EncodeBitDepth::Ten` / `encode_rgb16` route to `try_encode_frame_420_hbd` (verified in `src/encoder_svt_rs.rs`), and chunks 1+2 mean nothing on that path truncates: MD funnel, coded levels, and the deblock / CDEF / Wiener searches all read the native u16.

**What was missing was the EVIDENCE, not the code.** The chunk-2 gate is 100/100 — on synthetic content whose low 2 bits are a `(3r + 5c + v) % 4` pattern. That cannot represent photographic structure or a real transfer curve, and this repo has already been burned once by exactly that gap (`docs/bd10-port-map.md` records an 18/18 *photographic* failure while the synthetic bd10 gates were green).

So `identity_run` gained `SVTAV1_HBD_PQ`: the 8-bit luma is linearized as sRGB, mapped onto a 1000-nit display, run through the **SMPTE ST 2084 (PQ) OETF** and quantized to 10-bit limited range (64..940); chroma is rescaled 8-bit limited → 10-bit limited. The low bits are then a consequence of a nonlinear curve — no `<< 2` produces them — and the code-value histogram is PQ-shaped.

**Honest scope:** this is a PQ-shaped 10-bit code-value distribution derived from an 8-bit master, **not a native HDR capture**. Highlight detail an 8-bit master already clipped does not come back. What it tests is the 10-bit *sample* path on realistic code values. CICP is deliberately not varied — in mainline v4.2.0 the encode is CICP-independent apart from the header bits (the only reads of `transfer_characteristics` / `color_primaries` are the chroma-q boosts at `rc_crf_cqp.c:573-586`, inside `#if SVT_HDR_MODE`).

Two gates consume it:

| gate | grid | result |
|---|---|---|
| `bd10_hbd_src_gate.sh` PQ tier (synthetic, **runs in CI**) | {64,128} × qp {8,20,32} × preset {6,8,9} | **18/18 byte-identical** |
| `bd10_hbd_pq_gate.sh` (real CID22-512 photographs, **not in CI**) | 5 images × qp {8,20,32,55} × preset {6,8,9} | **presets 8 and 9: 40/40**; preset 6: 8/20 + 12 aarch64-scoped pins |

The PQ tier lives inside the CI-wired gate on purpose: no runner has the image corpora (`ZENAV1_SKIP_CORPUS_TESTS` at workflow scope), so a photographic gate can never answer a question on the x86-64 reference host — a synthetic one can. Preset 6 is in it deliberately: it is the only preset that runs the CDEF strength and Wiener LR searches, so it is where a 10-bit post-filter precision gap would show. It passes.

**12-bit: N/A.** C rejects it at `enc_settings.c:460`, so both sides reject it and there is no oracle. It becomes a gap only if the owner un-gates C first.

### The measurement that reframes bd10 parity — and it needs a decision

Running the bd10 gates on this macOS/clang/**aarch64** host against the same commit CI passes on x86-64:

| gate | CI (x86-64, Linux/gcc) | local (aarch64, macOS/clang) |
|---|---|---|
| `bd10_matrix.sh` (uniform) | 36/36 | 36/36 |
| `bd10_hbd_src_gate.sh` | 100/100 | 97/100 |
| `bd10_nonflat_gate.sh` | **309/309** | **197/309** |
| `bd10_photo_gate.sh` (not in CI) | — | **53/191** |

The port is not the variable side, checked rather than assumed: `tier_invariance.rs` holds its bytes constant across every archmage dispatch tier, and three failing photographic cells (`1484678 q12 p10`, `1001682 q12 p7`, `1001682 q12 p5`) were re-encoded by a build of the **pre-session tree** (`bfae1b69`) in a sibling workspace — byte-identical port output, still differing from local C.

This is `docs/SUSPECTED-C-BUGS.md` #9 (C's bitstream depends on the host build), but that entry's "~1240 cells, exactly one disagreement" is an **8-bit** figure. At bd10 the pattern is: flat and low-complexity synthetic content agrees on both hosts; non-flat and photographic content diverges. `bd10_nonflat_gate.sh` and `bd10_photo_gate.sh` therefore carry **no** aarch64 pins — 112 and 138 cells is not a pin list, it is a statement about the oracle, and pinning would bury it. Only the two hbd gates get `uname -m`-scoped pins (3 and 12).

**Decision needed from the owner:** an aarch64 CI runner would move ~250 bd10 cells from "unknown on half our hardware" to measured, and it is the only way to tell a real bd10 gap from this class without hand-running each cell. Until it exists, **a bd10 parity claim must name its host.** Entry #9 now carries the table and this recommendation.

Data: `benchmarks/bd10_pq_2026-08-28.md`. Commits: `d030fbf0` (PQ source + gates), `7fc16c51` (an unrelated FH desync this session introduced and fixed).

---
*By Claude (Anthropic) on the repo owner's instruction, 2026-08-28.*


## Historical comment — 2026-08-29T01:07:55Z

**Reference-host confirmation for the PQ tier — the preset-6 question is answered.**

The comment above could only claim the PQ tier passed on aarch64 and had to leave "is preset 6 a real 10-bit gap or the C-per-host class?" open, because no CI runner has the image corpora. CI run **33223502105** has now run the corpus-free PQ tier on x86-64:

```
✓ bd10 identity (uniform, port == real aomenc at bit depth 10)
✓ bd10 non-flat identity (u16 MD path, DC-family cells)
✓ bd10 NATIVE 10-bit source identity (task #6 chunk 2)     <- 115/115, PQ tier included
✓ bd10 partial-SB identity (10-bit at non-64-aligned dims)
```

So PQ-shaped 10-bit low bits are byte-identical to C at **preset 6 on both hosts** — including the preset that runs the CDEF strength and Wiener LR searches, which is where a 10-bit post-filter precision gap would have to show. That is the discriminator: the 12 aarch64 pins on `bd10_hbd_pq_gate.sh` are about photographic **content** on this host's C build, not about the low bits, and there is no 10-bit gap hiding behind them.

The same run also re-confirms the ISA table on the exact commit: `bd10 non-flat identity` is green on x86-64 while the same binary measures **197/309** locally on aarch64.

(That run's single red step was `Variance-boost delta-q recon parity` — a frame-header desync this session introduced in `5604a036` and fixed in `7fc16c51`, where CI is **green**. Details in the #9 thread and CHANGELOG; unrelated to the bd10 work above.)

---
*By Claude (Anthropic) on the repo owner's instruction, 2026-08-28.*

